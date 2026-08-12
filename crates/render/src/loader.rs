use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use domain::{PixelSize, WallpaperRef};
use tracing::{debug, warn};

use crate::decode::{self, Decoded};
use crate::error::RenderError;

/// Bytes discarded per read while clearing the readiness signal. One byte arrives per
/// finished decode, so this empties any plausible backlog in a single call.
const SIGNAL_CHUNK: usize = 64;

/// A wallpaper that finished decoding and is waiting to be uploaded.
pub struct Loaded {
    pub wallpaper: WallpaperRef,
    /// The size this was asked for. Carried back because an image smaller than the screen
    /// arrives smaller than that, and only the request records how much was tried.
    pub asked: PixelSize,
    pub decoded: Decoded,
}

/// Decoding and resizing, moved off the thread that draws.
///
/// A 4000x2250 image costs about 0.2 s to decode and resize, which on the event loop's
/// thread would be 0.2 s of frozen animation on every output. The result comes back
/// through a descriptor the loop can watch.
pub struct Loader {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// Readable while a finished decode has not been noticed yet.
    signal: UnixStream,
    /// What has been asked for and not yet answered, so a wallpaper is not queued again
    /// on every pass while its decode is still running.
    inflight: HashMap<WallpaperRef, PixelSize>,
    /// Wallpapers whose decode failed. Without this the next pass asks again, and an
    /// unreadable path is retried for as long as it stays configured.
    failed: HashSet<WallpaperRef>,
}

struct Job {
    wallpaper: WallpaperRef,
    size: PixelSize,
}

struct Done {
    wallpaper: WallpaperRef,
    asked: PixelSize,
    result: Result<Decoded, RenderError>,
}

impl Loader {
    pub fn new() -> Result<Self, RenderError> {
        let (signal, worker_signal) = UnixStream::pair()
            .map_err(|source| RenderError::Loader { operation: "socketpair", source })?;
        signal
            .set_nonblocking(true)
            .map_err(|source| RenderError::Loader { operation: "set_nonblocking", source })?;

        let (jobs, queue) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        thread::Builder::new()
            .name("decode".to_owned())
            .spawn(move || work(&queue, &finished, worker_signal))
            .map_err(|source| RenderError::Loader { operation: "spawn", source })?;

        Ok(Self { jobs, done, signal, inflight: HashMap::new(), failed: HashSet::new() })
    }

    /// Becomes readable when a decode finishes, which is what lets a wallpaper arriving
    /// long after the last frame still get one.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.signal.as_fd()
    }

    /// Queues a decode, unless the same work is already running or has already failed.
    pub fn request(&mut self, wallpaper: &WallpaperRef, size: PixelSize) {
        if self.failed.contains(wallpaper) {
            return;
        }
        if self.inflight.get(wallpaper).is_some_and(|queued| queued.covers(size)) {
            return;
        }

        debug!(
            path = %wallpaper.path().display(),
            width = size.w,
            height = size.h,
            "decoding wallpaper"
        );
        self.inflight.insert(wallpaper.clone(), size);
        // A closed channel means the decode thread is gone, which only happens on the way
        // out. The wallpaper stays as it is rather than the daemon failing over an image.
        let _ = self.jobs.send(Job { wallpaper: wallpaper.clone(), size });
    }

    /// Everything that finished since the last call.
    pub fn collect(&mut self) -> Vec<Loaded> {
        self.clear_signal();

        let mut ready = Vec::new();
        while let Ok(done) = self.done.try_recv() {
            self.inflight.remove(&done.wallpaper);
            match done.result {
                Ok(decoded) => {
                    ready.push(Loaded { wallpaper: done.wallpaper, asked: done.asked, decoded });
                }
                Err(error) => {
                    warn!(
                        path = %done.wallpaper.path().display(),
                        error = %crate::error::chain(&error),
                        "cannot show this wallpaper"
                    );
                    self.failed.insert(done.wallpaper);
                }
            }
        }
        ready
    }

    /// Drops the record of what failed for wallpapers nothing is showing any more, so
    /// setting a path again after fixing the file tries it once more.
    pub fn retain(&mut self, in_use: &HashSet<WallpaperRef>) {
        self.failed.retain(|wallpaper| in_use.contains(wallpaper));
    }

    /// Empties the readiness the event loop polled on, so a level-triggered descriptor
    /// stops spinning. The results themselves are taken by the next draw.
    pub fn clear_signal(&self) {
        let mut sink = [0u8; SIGNAL_CHUNK];
        loop {
            match (&self.signal).read(&mut sink) {
                Ok(0) => return,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    }
}

/// Decodes one image at a time, in the order asked for.
///
/// One thread rather than a pool: the worst case is two monitors changing wallpaper at
/// once, and a pool would cost twice the peak memory of a full-size decode to save a
/// moment on it.
fn work(jobs: &Receiver<Job>, finished: &Sender<Done>, mut signal: UnixStream) {
    while let Ok(job) = jobs.recv() {
        let result = decode::load(job.wallpaper.path(), job.size);
        let done = Done { wallpaper: job.wallpaper, asked: job.size, result };
        if finished.send(done).is_err() {
            return;
        }
        // After the result is queued, never before: the byte is what promises the main
        // thread that there is something to collect.
        if signal.write_all(&[0]).is_err() {
            return;
        }
    }
}
