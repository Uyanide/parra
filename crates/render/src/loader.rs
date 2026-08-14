use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use domain::{PixelSize, WallpaperRef};
use tracing::{debug, warn};

use crate::cache::{self, Cache};
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
    /// Set when this decode also wrote the resized copy, which the daemon records so the
    /// next start can use it.
    pub stored: bool,
    pub decoded: Decoded,
}

/// What one wallpaper failed at. Reported rather than only logged, so the daemon can put
/// the configured fallback on the outputs that were waiting for it.
pub struct Failed {
    pub wallpaper: WallpaperRef,
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
    /// Wallpapers whose decode failed. Without this the next pass asks again before the
    /// daemon has had a chance to react to the failure at all.
    failed: HashSet<WallpaperRef>,
    /// Where each remembered wallpaper's resized copy lives. A wallpaper absent from here
    /// is one nothing is remembering, and always decodes from its source.
    caches: HashMap<WallpaperRef, Cache>,
}

struct Job {
    wallpaper: WallpaperRef,
    size: PixelSize,
    cache: Option<Cache>,
}

struct Done {
    wallpaper: WallpaperRef,
    /// The size actually achieved, which for a copy read from disk is the size that copy
    /// was written for rather than the smaller one this job asked about.
    asked: PixelSize,
    stored: bool,
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

        Ok(Self {
            jobs,
            done,
            signal,
            inflight: HashMap::new(),
            failed: HashSet::new(),
            caches: HashMap::new(),
        })
    }

    /// Says where this wallpaper's resized copy belongs, and how large the one already
    /// there was asked for. `None` forgets it, which is what a transient set wants.
    pub fn set_cache(&mut self, wallpaper: &WallpaperRef, cache: Option<Cache>) {
        match cache {
            Some(cache) => self.caches.insert(wallpaper.clone(), cache),
            None => self.caches.remove(wallpaper),
        };
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
        let cache = self.caches.get(wallpaper).cloned();
        let _ = self.jobs.send(Job { wallpaper: wallpaper.clone(), size, cache });
    }

    /// Everything that finished since the last call, and everything that gave up.
    pub fn collect(&mut self) -> (Vec<Loaded>, Vec<Failed>) {
        self.clear_signal();

        let (mut ready, mut lost) = (Vec::new(), Vec::new());
        while let Ok(done) = self.done.try_recv() {
            self.inflight.remove(&done.wallpaper);
            match done.result {
                Ok(decoded) => ready.push(Loaded {
                    wallpaper: done.wallpaper,
                    asked: done.asked,
                    stored: done.stored,
                    decoded,
                }),
                Err(error) => {
                    warn!(
                        path = %done.wallpaper.path().display(),
                        error = %crate::error::chain(&error),
                        "cannot show this wallpaper"
                    );
                    self.failed.insert(done.wallpaper.clone());
                    lost.push(Failed { wallpaper: done.wallpaper });
                }
            }
        }
        (ready, lost)
    }

    /// Drops what is remembered about wallpapers nothing is showing any more, so setting
    /// a path again after fixing the file tries it once more.
    pub fn retain(&mut self, in_use: &HashSet<WallpaperRef>) {
        self.failed.retain(|wallpaper| in_use.contains(wallpaper));
        self.caches.retain(|wallpaper, _| in_use.contains(wallpaper));
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
        let done = run(job);
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

/// Serves one job from the resized copy when there is one large enough, and from the
/// source otherwise.
///
/// A copy that will not read is never fatal: it falls through to the source, which is
/// what makes a deleted or truncated cache file recover on its own. Only both failing is
/// a failure.
fn run(job: Job) -> Done {
    let source = job.wallpaper.path();
    let usable = job.cache.as_ref().filter(|cache| cache.serves(job.size));

    if let Some(cache) = usable {
        match cache::read(&cache.file) {
            // The size the copy was written for, not the smaller one this job asked
            // about: understating it would force a source decode the next time the
            // output grew back to what the copy already covers.
            Ok(decoded) => {
                let asked = cache.asked.unwrap_or(job.size);
                debug!(path = %source.display(), "restored a wallpaper from its cached copy");
                return Done {
                    wallpaper: job.wallpaper,
                    asked,
                    stored: false,
                    result: Ok(decoded),
                };
            }
            Err(error) => warn!(
                path = %cache.file.display(),
                error = %crate::error::chain(&error),
                "cached copy unusable, decoding the original again"
            ),
        }
    }

    let decoded = match decode::load(source, job.size) {
        Ok(decoded) => decoded,
        Err(error) => {
            return Done {
                wallpaper: job.wallpaper,
                asked: job.size,
                stored: false,
                result: Err(error),
            };
        }
    };

    // A copy that cannot be written costs the next start its head start and nothing else,
    // so it is logged where it happens and the wallpaper still goes up.
    let stored = job.cache.is_some_and(|cache| match cache::write(&cache.file, &decoded) {
        Ok(()) => true,
        Err(error) => {
            warn!(error = %crate::error::chain(&error), "cannot keep a copy of this wallpaper");
            false
        }
    });
    Done { wallpaper: job.wallpaper, asked: job.size, stored, result: Ok(decoded) }
}
