use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use inotify::{Inotify, WatchDescriptor, WatchMask};
use thiserror::Error;

/// Every way a file can be replaced, plus the two ways it can disappear. A configuration
/// that was deleted is still a change: it means the built-in defaults now apply.
///
/// `CREATE` is what a link being re-pointed looks like: `symlink` and `link` produce it
/// and nothing else.
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

/// One watched directory and the name in it that is ours.
struct Target {
    wd: WatchDescriptor,
    directory: PathBuf,
    name: OsString,
}

/// Notices edits to the configuration file.
///
/// Watches directories rather than the file itself, and follows the file where a link
/// points. Both rules are in `docs/architecture.md`.
pub struct Watcher {
    inotify: Inotify,
    /// The path as configured, which is what gets resolved again after every event.
    config: PathBuf,
    /// The directory the file is named in, then the one it resolves to when that name is
    /// a link.
    targets: Vec<Target>,
    buffer: [u8; CAPACITY],
}

impl Watcher {
    pub fn new(config: &Path) -> Result<Self, WatchError> {
        let failed = |source| WatchError { path: config.to_owned(), source };
        let inotify = Inotify::init().map_err(failed)?;
        let mut watcher =
            Self { inotify, config: config.to_owned(), targets: Vec::new(), buffer: [0; CAPACITY] };
        watcher.resolve().map_err(failed)?;
        Ok(watcher)
    }

    /// Whether the configuration file itself changed. Always drains what the kernel had
    /// queued, so unrelated files in the same directory cannot accumulate.
    pub fn changed(&mut self) -> bool {
        let ours = {
            let Ok(events) = self.inotify.read_events(&mut self.buffer) else {
                return false;
            };
            let targets = &self.targets;
            let mine = |event: &inotify::Event<&OsStr>| {
                targets.iter().any(|target| {
                    target.wd == event.wd && event.name == Some(target.name.as_os_str())
                })
            };
            events.filter(mine).count() > 0
        };
        if ours {
            // The event may itself have been a link re-pointed, in which case the file to
            // watch from now on is a different one.
            let _ = self.resolve();
        }
        ours
    }

    /// Points the watches at wherever the configured path leads now.
    ///
    /// The directory the file is named in must be watchable; the one a link leads to is
    /// skipped when it is not, and tried again on the next event.
    fn resolve(&mut self) -> io::Result<()> {
        let places = places(&self.config);
        let unchanged = self.targets.len() == places.len()
            && (self.targets.iter()).zip(&places).all(|(target, (directory, name))| {
                target.directory == *directory && target.name == *name
            });
        if unchanged {
            return Ok(());
        }

        let mut targets = Vec::with_capacity(places.len());
        for (rank, (directory, name)) in places.into_iter().enumerate() {
            // A second `add` on a directory already watched answers with the watch that
            // exists, so removing it later would take both names down with it.
            let held = (self.targets.iter())
                .find(|target| target.directory == directory)
                .map(|target| target.wd.clone());
            let wd = match held {
                Some(wd) => wd,
                None => match self.inotify.watches().add(&directory, EVENTS) {
                    Ok(wd) => wd,
                    Err(source) if rank == 0 => return Err(source),
                    Err(_) => continue,
                },
            };
            targets.push(Target { wd, directory, name });
        }

        for old in &self.targets {
            if !targets.iter().any(|target| target.directory == old.directory) {
                let _ = self.inotify.watches().remove(old.wd.clone());
            }
        }
        self.targets = targets;
        Ok(())
    }
}

/// Which directories carry the events that are ours, and the name to look for in each.
///
/// The parent is resolved on its own so that a file which does not exist yet is still
/// watched, and the file is resolved separately to follow a link into another directory.
fn places(config: &Path) -> Vec<(PathBuf, OsString)> {
    let named = config.parent().unwrap_or(Path::new("."));
    let named = named.canonicalize().unwrap_or_else(|_| named.to_owned());
    let name = config.file_name().unwrap_or(OsStr::new("")).to_owned();
    let mut places = vec![(named, name)];

    if let Ok(real) = config.canonicalize()
        && let (Some(directory), Some(name)) = (real.parent(), real.file_name())
        && (directory, name) != (places[0].0.as_path(), places[0].1.as_os_str())
    {
        places.push((directory.to_owned(), name.to_owned()));
    }
    places
}

/// Readable when something in a watched directory changed, so an event loop can wait on
/// it rather than anything having to poll.
impl AsFd for Watcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
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
    fn a_linked_configuration_directory_is_watched_where_it_leads() {
        let dir = directory();
        let real = dir.join("dotfiles");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("config.toml"), "[general]\n").unwrap();
        let link = dir.join("config");
        symlink(&real, &link).unwrap();

        let mut watcher = Watcher::new(&link.join("config.toml")).unwrap();
        fs::write(real.join("config.toml"), "[general]\nlayer = \"bottom\"\n").unwrap();
        assert!(waited_for(&mut watcher), "the edit lands in the directory the link leads to");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_linked_configuration_file_is_watched_where_it_leads() {
        let dir = directory();
        let store = dir.join("dotfiles");
        let config_dir = dir.join("config");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(&config_dir).unwrap();
        let real = store.join("config.toml");
        fs::write(&real, "[general]\n").unwrap();
        let link = config_dir.join("config.toml");
        symlink(&real, &link).unwrap();

        let mut watcher = Watcher::new(&link).unwrap();
        fs::write(&real, "[general]\nlayer = \"bottom\"\n").unwrap();
        assert!(waited_for(&mut watcher), "an edit through the link is ours");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// What a dotfile manager does on the way to a new generation.
    #[test]
    fn a_link_that_is_re_pointed_is_followed_to_its_new_target() {
        let dir = directory();
        let store = dir.join("dotfiles");
        let config_dir = dir.join("config");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(&config_dir).unwrap();
        let first = store.join("first.toml");
        let second = store.join("second.toml");
        fs::write(&first, "[general]\n").unwrap();
        fs::write(&second, "[general]\n").unwrap();
        let link = config_dir.join("config.toml");
        symlink(&first, &link).unwrap();

        let mut watcher = Watcher::new(&link).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        assert!(waited_for(&mut watcher), "the link itself was replaced");

        fs::write(&second, "[general]\nlayer = \"bottom\"\n").unwrap();
        assert!(waited_for(&mut watcher), "edits now follow the new target");

        fs::write(&first, "[general]\nlayer = \"background\"\n").unwrap();
        assert!(!reported_within(&mut watcher, GRACE), "the old target is no longer ours");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error_rather_than_a_silent_no_op() {
        let path = std::env::temp_dir().join("watch-nowhere-at-all").join("config.toml");
        assert!(Watcher::new(&path).is_err());
    }
}
