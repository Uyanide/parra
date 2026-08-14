use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use domain::{OutputId, PixelSize, WallpaperRef};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

/// Format of the file on disk. A file claiming anything else is ignored rather than
/// guessed at.
pub const VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum StateError {
    #[error("cannot read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot parse {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot serialize the state file")]
    Serialize(#[source] toml::ser::Error),
}

/// What the daemon remembers between runs: which wallpaper each slot was asked for, and
/// how large a copy has been kept for it.
///
/// Written by the daemon and not meant to be hand-edited, which is why unknown keys are
/// tolerated here and rejected in the configuration file: loud rejection helps someone
/// editing a file and only hurts a machine reading its own.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct State {
    pub version: u32,
    /// Shown by every output without one of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wallpaper: Option<Entry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    output: BTreeMap<OutputId, Entry>,
}

impl Default for State {
    fn default() -> Self {
        Self { version: VERSION, wallpaper: None, output: BTreeMap::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Entry {
    pub source: PathBuf,
    pub epoch: u64,
    /// Size the copy was **asked** for, absent until one has been written.
    ///
    /// Asked rather than produced: an image smaller than the screen is never enlarged, so
    /// recording what came back would never satisfy the request that produced it and the
    /// source would be re-decoded on every pass. As `[w, h]`, since the derived form of a
    /// size is a table and would put one in the middle of every entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked: Option<[u32; 2]>,
}

impl Entry {
    pub fn new(wallpaper: &WallpaperRef) -> Self {
        Self { source: wallpaper.path().to_owned(), epoch: wallpaper.epoch(), asked: None }
    }

    /// The wallpaper this entry stands for, identity and all, so what the daemon restores
    /// is indistinguishable from what it recorded.
    pub fn wallpaper(&self) -> WallpaperRef {
        WallpaperRef::at(self.source.clone(), self.epoch)
    }

    pub fn asked(&self) -> Option<PixelSize> {
        self.asked.map(|[w, h]| PixelSize::new(w, h))
    }

    pub fn set_asked(&mut self, asked: PixelSize) {
        self.asked = Some([asked.w, asked.h]);
    }
}

impl State {
    /// A missing file is not an error: nothing has been set yet, which is a valid state
    /// and the one every first run is in.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(StateError::Read { path: path.to_owned(), source }),
        };

        let state: Self = toml::from_str(&text)
            .map_err(|source| StateError::Parse { path: path.to_owned(), source })?;

        if state.version != VERSION {
            warn!(
                path = %path.display(),
                found = state.version,
                expected = VERSION,
                "ignoring a state file written by another version"
            );
            return Ok(Self::default());
        }
        Ok(state)
    }

    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let text = toml::to_string(self).map_err(StateError::Serialize)?;
        crate::atomic::write(path, text.as_bytes())
            .map_err(|source| StateError::Write { path: path.to_owned(), source })
    }

    /// Records what one slot is showing, or that it is showing nothing of its own.
    ///
    /// `None` for the output addresses every output and therefore drops the per-output
    /// entries, exactly as `domain::Signals::set_wallpaper` does: the two have to agree,
    /// or a broadcast would leave per-output copies behind that nothing points at.
    /// `None` for the entry empties the slot rather than filling it, which is what makes
    /// an override removable without a second addressing rule.
    pub fn set(&mut self, output: Option<&OutputId>, entry: Option<Entry>) {
        match output {
            None => {
                self.wallpaper = entry;
                self.output.clear();
            }
            Some(id) => match entry {
                Some(entry) => {
                    self.output.insert(id.clone(), entry);
                }
                None => {
                    self.output.remove(id);
                }
            },
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = (Option<&OutputId>, &Entry)> {
        let global = self.wallpaper.iter().map(|entry| (None, entry));
        global.chain(self.output.iter().map(|(id, entry)| (Some(id), entry)))
    }

    /// The entry for one wallpaper, whichever slot holds it.
    pub fn entry_mut(&mut self, wallpaper: &WallpaperRef) -> Option<&mut Entry> {
        let global = self.wallpaper.iter_mut();
        global.chain(self.output.values_mut()).find(|entry| &entry.wallpaper() == wallpaper)
    }

    /// Highest epoch on record, so the next one allocated cannot collide with a file an
    /// earlier run left behind.
    pub fn max_epoch(&self) -> u64 {
        self.entries().map(|(_, entry)| entry.epoch).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn directory() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("state-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn populated() -> State {
        let mut state = State::default();
        let mut global = Entry::new(&WallpaperRef::at("/srv/a.png", 7));
        global.set_asked(PixelSize::new(3556, 2001));
        state.set(None, Some(global));
        state.set(
            Some(&OutputId::new("DP-1")),
            Some(Entry::new(&WallpaperRef::at("/srv/b.png", 8))),
        );
        state
    }

    #[test]
    fn a_state_file_survives_a_round_trip() {
        let dir = directory();
        let file = dir.join("state.toml");
        let state = populated();
        state.save(&file).unwrap();

        let read = State::load(&file).unwrap();
        assert_eq!(read.entries().count(), 2);
        assert_eq!(
            read.entries().map(|(_, entry)| entry.clone()).collect::<Vec<_>>(),
            state.entries().map(|(_, entry)| entry.clone()).collect::<Vec<_>>()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_entry_with_no_copy_yet_omits_the_size() {
        let mut state = State::default();
        state.set(None, Some(Entry::new(&WallpaperRef::at("/srv/a.png", 1))));
        let text = toml::to_string(&state).unwrap();

        assert!(!text.contains("asked"), "{text}");
        let read: State = toml::from_str(&text).unwrap();
        assert_eq!(read.entries().next().unwrap().1.asked(), None);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = directory();
        let state = State::load(&dir.join("absent.toml")).unwrap();
        assert_eq!(state.entries().count(), 0);
        assert_eq!(state.version, VERSION);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_from_another_version_is_ignored_rather_than_fatal() {
        let dir = directory();
        let file = dir.join("state.toml");
        fs::write(&file, "version = 999\n[wallpaper]\nsource = \"/srv/a.png\"\nepoch = 1\n")
            .unwrap();

        let state = State::load(&file).unwrap();
        assert_eq!(state.entries().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unparseable_file_is_reported() {
        let dir = directory();
        let file = dir.join("state.toml");
        fs::write(&file, "this is not toml").unwrap();
        assert!(matches!(State::load(&file), Err(StateError::Parse { .. })));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broadcast_drops_the_per_output_entries() {
        let mut state = populated();
        state.set(None, Some(Entry::new(&WallpaperRef::at("/srv/c.png", 9))));

        let sources: Vec<&Path> =
            state.entries().map(|(_, entry)| entry.source.as_path()).collect();
        assert_eq!(sources, [Path::new("/srv/c.png")], "the DP-1 copy would be orphaned");
    }

    #[test]
    fn the_highest_epoch_is_what_the_next_one_follows() {
        assert_eq!(populated().max_epoch(), 8);
        assert_eq!(State::default().max_epoch(), 0);
    }

    #[test]
    fn a_size_is_recorded_against_the_wallpaper_that_was_asked_for() {
        let mut state = populated();
        let wallpaper = WallpaperRef::at("/srv/b.png", 8);
        state.entry_mut(&wallpaper).unwrap().set_asked(PixelSize::new(3200, 1800));

        assert_eq!(state.entry_mut(&wallpaper).unwrap().asked(), Some(PixelSize::new(3200, 1800)));
        // The same path at another epoch is another wallpaper, and has no entry.
        assert!(state.entry_mut(&WallpaperRef::at("/srv/b.png", 9)).is_none());
    }
}
