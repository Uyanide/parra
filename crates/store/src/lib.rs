//! What the daemon remembers between runs: the wallpaper each slot was asked for, and a
//! resized copy of it so the next start does not pay for the decode again.
//!
//! Both locations arrive as parameters, so the program's name has no definition point
//! here. `config` stays about the user's file; this is about the daemon's own.

pub mod atomic;
pub mod naming;
pub mod state;

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{fs, io};

use domain::{OutputId, PixelSize, WallpaperRef};
use thiserror::Error;
use tracing::{debug, warn};

pub use state::{Entry, State, StateError};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("cannot create {}", path.display())]
    Directory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    State(#[from] StateError),
}

/// The state file, the directory of resized copies, and the counter that names them.
///
/// Every wallpaper the control socket sets gets an epoch of its own. It is part of the
/// wallpaper's identity, so setting the same path twice reloads everything without any
/// cache being told to invalidate, and it is what makes each copy's filename unique, so a
/// file written for an earlier wallpaper can never be taken for the current one.
pub struct Store {
    file: PathBuf,
    cache_dir: PathBuf,
    state: State,
    next_epoch: u64,
}

impl Store {
    /// Loads what the last run left, creates both directories, and clears any copy the
    /// state file no longer points at.
    ///
    /// A state file that will not parse is a warning rather than a failure: starting on
    /// the configured fallback and leaving the file for inspection beats not starting.
    pub fn open(file: &Path, cache_dir: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = file.parent() {
            create_dir(parent)?;
        }
        create_dir(cache_dir)?;

        let state = State::load(file).unwrap_or_else(|error| {
            warn!(error = %chain(&error), "cannot read the remembered wallpaper");
            State::default()
        });

        let store = Self {
            file: file.to_owned(),
            cache_dir: cache_dir.to_owned(),
            next_epoch: state.max_epoch() + 1,
            state,
        };
        store.sweep();
        Ok(store)
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Every wallpaper the last run was showing, with the slot that asked for it.
    pub fn entries(&self) -> impl Iterator<Item = (Option<&OutputId>, &Entry)> {
        self.state.entries()
    }

    /// A wallpaper identity that nothing else will ever be given, and that nothing is
    /// recorded against. For a set the caller does not want remembered.
    pub fn transient(&mut self, source: PathBuf) -> WallpaperRef {
        WallpaperRef::at(source, self.allocate())
    }

    /// Records what a slot is now showing. The caller still has to `save`, so that a set
    /// addressing every output writes the file once rather than once per monitor.
    pub fn set(&mut self, output: Option<&OutputId>, source: PathBuf) -> WallpaperRef {
        let wallpaper = WallpaperRef::at(source, self.allocate());
        self.state.set(output, Some(Entry::new(&wallpaper)));
        wallpaper
    }

    /// Stops remembering anything for a slot. The output then shows whatever it would
    /// have shown had nothing ever been set for it, which is the global entry or the
    /// configured fallback; `None` forgets every slot at once.
    ///
    /// The copy is not deleted here. `save` sweeps whatever the file stops naming, so the
    /// two cannot disagree about which files are still wanted.
    pub fn clear(&mut self, output: Option<&OutputId>) {
        self.state.set(output, None);
    }

    /// Notes that a copy now exists for this wallpaper, at the size it was asked for.
    /// Unknown wallpapers are ignored: a transient set has no entry by design.
    pub fn stored(&mut self, wallpaper: &WallpaperRef, asked: PixelSize) -> bool {
        match self.state.entry_mut(wallpaper) {
            Some(entry) => {
                entry.set_asked(asked);
                true
            }
            None => false,
        }
    }

    /// Writes the state file, then clears every copy it no longer points at.
    ///
    /// In that order: a sweep is driven entirely by what the file says, so running it
    /// before the write would keep the previous set's copies and drop the current one's.
    pub fn save(&mut self) -> Result<(), StoreError> {
        self.state.save(&self.file)?;
        self.sweep();
        Ok(())
    }

    /// Where this wallpaper's resized copy belongs, and how large the one on disk was
    /// asked for. `None` for a wallpaper nothing is remembering.
    pub fn cached(&self, wallpaper: &WallpaperRef) -> Option<(PathBuf, Option<PixelSize>)> {
        let (output, entry) = self.entries().find(|(_, entry)| &entry.wallpaper() == wallpaper)?;
        Some((self.cache_dir.join(naming::cache_file(output, entry.epoch)), entry.asked()))
    }

    fn allocate(&mut self) -> u64 {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        epoch
    }

    /// Deletes every file in the cache directory that no entry names, which is both what
    /// clears the previous wallpaper and what removes anything an interrupted write left.
    fn sweep(&self) {
        let keep: HashSet<OsString> = self
            .entries()
            .map(|(output, entry)| OsString::from(naming::cache_file(output, entry.epoch)))
            .collect();
        debug!(dir = %self.cache_dir.display(), keeping = keep.len(), "sweeping cached wallpapers");
        atomic::sweep(&self.cache_dir, &keep);
    }
}

fn create_dir(path: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(path)
        .map_err(|source| StoreError::Directory { path: path.to_owned(), source })
}

/// Renders an error together with what caused it, since only the outermost message would
/// otherwise reach a log line and the cause is the half worth reading.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        text.push_str(": ");
        text.push_str(&source.to_string());
        cause = source.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    struct Fixture {
        root: PathBuf,
        store: Store,
    }

    impl Fixture {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("store-{}-{unique}", std::process::id()));
            let store = Store::open(&root.join("state.toml"), &root.join("cache")).unwrap();
            Self { root, store }
        }

        /// Reopens, as a restart would.
        fn restart(&mut self) {
            self.store =
                Store::open(&self.root.join("state.toml"), &self.root.join("cache")).unwrap();
        }

        /// Stands in for the decode thread writing a copy.
        fn write_copy(&self, wallpaper: &WallpaperRef) {
            let (file, _) = self.store.cached(wallpaper).expect("a recorded wallpaper");
            fs::write(file, b"pretend this is QOI").unwrap();
        }

        fn cached_files(&self) -> Vec<String> {
            let mut found: Vec<String> = fs::read_dir(self.root.join("cache"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            found.sort();
            found
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn opening_creates_both_directories() {
        let fixture = Fixture::new();
        assert!(fixture.root.join("cache").is_dir());
        assert!(fixture.store.file().parent().unwrap().is_dir());
    }

    #[test]
    fn a_wallpaper_survives_a_restart_with_its_identity() {
        let mut fixture = Fixture::new();
        let wallpaper = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        fixture.store.stored(&wallpaper, PixelSize::new(2560, 1440));
        fixture.store.save().unwrap();
        fixture.write_copy(&wallpaper);

        fixture.restart();
        let (output, entry) = fixture.store.entries().next().expect("one entry");
        assert_eq!(output, None);
        assert_eq!(entry.wallpaper(), wallpaper);
        assert_eq!(entry.asked(), Some(PixelSize::new(2560, 1440)));
        assert_eq!(fixture.cached_files().len(), 1, "the copy should have survived the sweep");
    }

    #[test]
    fn epochs_never_repeat_across_a_restart() {
        let mut fixture = Fixture::new();
        let first = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        fixture.store.save().unwrap();

        fixture.restart();
        let second = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        assert!(second.epoch() > first.epoch(), "{second:?} must not reuse {first:?}");
        assert_ne!(first, second, "the same path set twice is two wallpapers");
    }

    #[test]
    fn a_new_wallpaper_sweeps_the_one_it_replaced() {
        let mut fixture = Fixture::new();
        let first = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        fixture.store.save().unwrap();
        fixture.write_copy(&first);
        assert_eq!(fixture.cached_files().len(), 1);

        let second = fixture.store.set(None, PathBuf::from("/srv/b.png"));
        fixture.store.save().unwrap();
        assert!(fixture.cached_files().is_empty(), "the old copy should be gone at once");

        fixture.write_copy(&second);
        assert_eq!(fixture.cached_files().len(), 1);
    }

    #[test]
    fn a_broadcast_sweeps_the_per_output_copies() {
        let mut fixture = Fixture::new();
        let dp1 = OutputId::new("DP-1");
        let single = fixture.store.set(Some(&dp1), PathBuf::from("/srv/b.png"));
        fixture.store.save().unwrap();
        fixture.write_copy(&single);

        let all = fixture.store.set(None, PathBuf::from("/srv/c.png"));
        fixture.store.save().unwrap();
        fixture.write_copy(&all);

        assert_eq!(fixture.cached_files().len(), 1, "the DP-1 copy is no longer pointed at");
    }

    #[test]
    fn clearing_one_output_sweeps_its_copy_and_keeps_the_rest() {
        let mut fixture = Fixture::new();
        let dp1 = OutputId::new("DP-1");
        let all = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        let single = fixture.store.set(Some(&dp1), PathBuf::from("/srv/b.png"));
        fixture.store.save().unwrap();
        fixture.write_copy(&all);
        fixture.write_copy(&single);
        assert_eq!(fixture.cached_files().len(), 2);

        fixture.store.clear(Some(&dp1));
        fixture.store.save().unwrap();

        let left: Vec<WallpaperRef> =
            fixture.store.entries().map(|(_, entry)| entry.wallpaper()).collect();
        assert_eq!(left, [all], "DP-1 falls back to the broadcast entry");
        assert_eq!(fixture.cached_files().len(), 1, "only the override's copy should go");
    }

    #[test]
    fn clearing_everything_empties_the_file_and_the_directory() {
        let mut fixture = Fixture::new();
        let all = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        let single = fixture.store.set(Some(&OutputId::new("DP-1")), PathBuf::from("/srv/b.png"));
        fixture.store.save().unwrap();
        fixture.write_copy(&all);
        fixture.write_copy(&single);

        fixture.store.clear(None);
        fixture.store.save().unwrap();
        assert!(fixture.cached_files().is_empty());

        fixture.restart();
        assert_eq!(fixture.store.entries().count(), 0, "a clear has to outlive the process too");
    }

    #[test]
    fn clearing_a_slot_that_holds_nothing_is_harmless() {
        let mut fixture = Fixture::new();
        let all = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        fixture.store.save().unwrap();
        fixture.write_copy(&all);

        fixture.store.clear(Some(&OutputId::new("DP-1")));
        fixture.store.save().unwrap();

        assert_eq!(fixture.store.entries().next().unwrap().1.wallpaper(), all);
        assert_eq!(fixture.cached_files().len(), 1);
    }

    #[test]
    fn a_transient_wallpaper_is_recorded_nowhere() {
        let mut fixture = Fixture::new();
        let kept = fixture.store.set(None, PathBuf::from("/srv/a.png"));
        fixture.store.save().unwrap();

        let passing = fixture.store.transient(PathBuf::from("/srv/d.png"));
        assert!(fixture.store.cached(&passing).is_none());
        assert!(!fixture.store.stored(&passing, PixelSize::new(100, 100)));

        fixture.restart();
        assert_eq!(fixture.store.entries().next().unwrap().1.wallpaper(), kept);
    }

    #[test]
    fn an_unreadable_state_file_starts_from_nothing_and_is_left_alone() {
        let fixture = Fixture::new();
        let file = fixture.root.join("state.toml");
        fs::write(&file, "this is not toml").unwrap();

        let store = Store::open(&file, &fixture.root.join("cache")).unwrap();
        assert_eq!(store.entries().count(), 0);
        assert_eq!(fs::read_to_string(&file).unwrap(), "this is not toml");
    }
}
