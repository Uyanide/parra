use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::Path;

use tracing::{debug, warn};

/// Suffix of the file a write goes to before it is renamed into place.
const PENDING: &str = ".tmp";

/// Replaces `path` with `bytes`, or leaves whatever was there untouched.
///
/// The temporary file is a sibling rather than one in the system temp directory, because
/// a rename is only atomic within a filesystem. `sync_all` before the rename is what
/// stops a crash leaving a correctly named file full of nothing.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let pending = pending_path(path);
    let mut file = File::create(&pending)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);

    if let Err(error) = written {
        let _ = fs::remove_file(&pending);
        return Err(error);
    }
    fs::rename(&pending, path)
}

/// Deletes every regular file in `dir` that is not named in `keep`.
///
/// Errors are logged rather than returned: a file left behind costs disk space and
/// nothing else, so it is never worth failing a wallpaper change over.
pub fn sweep(dir: &Path, keep: &HashSet<OsString>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            warn!(dir = %dir.display(), %error, "cannot list the cache directory");
            return;
        }
    };

    for entry in entries.flatten() {
        if keep.contains(&entry.file_name()) {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        match fs::remove_file(&path) {
            Ok(()) => debug!(path = %path.display(), "swept an unused cached wallpaper"),
            Err(error) => warn!(path = %path.display(), %error, "cannot remove"),
        }
    }
}

/// Sibling of `path` that a write goes to first. Kept out of `keep` sets by its suffix,
/// so a sweep clears anything an interrupted write left behind.
fn pending_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_else(|| OsStr::new("state")).to_owned();
    name.push(PENDING);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn directory() -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("atomic-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_write_leaves_nothing_behind() {
        let dir = directory();
        let file = dir.join("state.toml");
        write(&file, b"version = 1\n").unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"version = 1\n");
        assert_eq!(names(&dir), ["state.toml"], "the temporary file should be gone");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_that_cannot_start_leaves_the_original_intact() {
        let dir = directory();
        let file = dir.join("state.toml");
        write(&file, b"first").unwrap();

        // A directory cannot be created where the temporary file has to go.
        fs::create_dir(dir.join("state.toml.tmp")).unwrap();
        assert!(write(&file, b"second").is_err());

        assert_eq!(fs::read(&file).unwrap(), b"first");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sweep_keeps_what_is_named_and_removes_the_rest() {
        let dir = directory();
        for name in ["keep.qoi", "orphan.qoi", "orphan.qoi.tmp"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        fs::create_dir(dir.join("subdir")).unwrap();

        sweep(&dir, &HashSet::from([OsString::from("keep.qoi")]));

        assert_eq!(names(&dir), ["keep.qoi", "subdir"], "only directories and the keep set stay");
        fs::remove_dir_all(&dir).unwrap();
    }
}
