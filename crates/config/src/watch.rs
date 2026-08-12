use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use inotify::{Inotify, WatchMask};
use thiserror::Error;

/// Every way a file can be replaced, plus the two ways it can disappear. A configuration
/// that was deleted is still a change: it means the built-in defaults now apply.
const EVENTS: WatchMask = WatchMask::CLOSE_WRITE
    .union(WatchMask::CREATE)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::DELETE);

/// Room for a burst of events on one directory. Anything that does not fit stays queued
/// in the kernel and arrives on the next read, so the size is a throughput choice only.
const CAPACITY: usize = 4096;

#[derive(Debug, Error)]
#[error("cannot watch {} for changes", path.display())]
pub struct WatchError {
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}

/// Notices edits to the configuration file.
///
/// Watches the containing directory rather than the file itself. An editor writes a
/// temporary file and renames it over the original, so a watch on the file would follow
/// the inode that was replaced and never hear about the name that matters.
pub struct Watcher {
    inotify: Inotify,
    /// Which name in that directory is ours.
    file: OsString,
    buffer: [u8; CAPACITY],
}

impl Watcher {
    pub fn new(config: &Path) -> Result<Self, WatchError> {
        let directory = config.parent().unwrap_or(Path::new("."));
        let file = config.file_name().unwrap_or(OsStr::new("")).to_owned();
        let failed = |source| WatchError { path: config.to_owned(), source };

        let inotify = Inotify::init().map_err(failed)?;
        inotify.watches().add(directory, EVENTS).map_err(failed)?;
        Ok(Self { inotify, file, buffer: [0; CAPACITY] })
    }

    /// Whether the configuration file itself changed. Always drains what the kernel had
    /// queued, so unrelated files in the same directory cannot accumulate.
    pub fn changed(&mut self) -> bool {
        let Ok(events) = self.inotify.read_events(&mut self.buffer) else {
            return false;
        };
        let file = self.file.as_os_str();
        events.filter(|event| event.name == Some(file)).count() > 0
    }
}

/// Readable when something in the watched directory changed, so an event loop can wait on
/// it rather than anything having to poll.
impl AsFd for Watcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// Long enough that a loaded machine still delivers, short enough to fail rather than
    /// hang. Events normally arrive before the write call has even returned.
    const PATIENCE: Duration = Duration::from_secs(2);

    /// How long a change that must not be reported is given to show up anyway.
    const GRACE: Duration = Duration::from_millis(200);

    fn directory() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("watch-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn reported_within(watcher: &mut Watcher, patience: Duration) -> bool {
        let deadline = Instant::now() + patience;
        while Instant::now() < deadline {
            if watcher.changed() {
                return true;
            }
            std::thread::yield_now();
        }
        false
    }

    fn waited_for(watcher: &mut Watcher) -> bool {
        reported_within(watcher, PATIENCE)
    }

    #[test]
    fn an_edit_is_noticed() {
        let dir = directory();
        let config = dir.join("config.toml");
        fs::write(&config, "[general]\n").unwrap();

        let mut watcher = Watcher::new(&config).unwrap();
        fs::write(&config, "[general]\nlayer = \"bottom\"\n").unwrap();
        assert!(waited_for(&mut watcher));

        assert!(!watcher.changed(), "the queue should be empty once it has been read");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_does_not_exist_yet_can_still_be_watched() {
        let dir = directory();
        let config = dir.join("config.toml");

        let mut watcher = Watcher::new(&config).unwrap();
        fs::write(&config, "[general]\n").unwrap();
        assert!(waited_for(&mut watcher));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_neighbouring_file_is_not_our_business() {
        let dir = directory();
        let config = dir.join("config.toml");
        fs::write(&config, "[general]\n").unwrap();

        let mut watcher = Watcher::new(&config).unwrap();
        fs::write(dir.join("config.toml.swp"), "editor droppings").unwrap();
        fs::write(dir.join("notes.txt"), "unrelated").unwrap();
        assert!(!reported_within(&mut watcher, GRACE), "a neighbour is not a reload");

        // And having read past them, the edit that is ours still gets through.
        fs::write(&config, "[general]\nlayer = \"bottom\"\n").unwrap();
        assert!(waited_for(&mut watcher));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error_rather_than_a_silent_no_op() {
        let path = std::env::temp_dir().join("watch-nowhere-at-all").join("config.toml");
        assert!(Watcher::new(&path).is_err());
    }
}
