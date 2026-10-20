use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static SAVE_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct FolderEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Default)]
pub struct LibraryFolders {
    roots: Vec<PathBuf>,
    preferences_path: Option<PathBuf>,
    listings: HashMap<PathBuf, Result<Vec<FolderEntry>, String>>,
}

#[derive(Deserialize, Serialize)]
struct Preferences {
    roots: Vec<PathBuf>,
}

impl LibraryFolders {
    pub fn load_user_preferences(&mut self) -> Result<(), String> {
        self.load_preferences_from(preferences_path()?)
    }

    fn load_preferences_from(&mut self, path: PathBuf) -> Result<(), String> {
        self.preferences_path = Some(path.clone());
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.roots.clear();
                self.refresh();
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "Could not read Custom folders from {}: {error}",
                    path.display()
                ));
            }
        };
        let preferences: Preferences = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Could not read Custom folders from {}: {error}",
                path.display()
            )
        })?;
        let mut seen = HashSet::new();
        self.roots = preferences
            .roots
            .into_iter()
            .map(|root| root.canonicalize().unwrap_or(root))
            .filter(|root| seen.insert(root.clone()))
            .collect();
        self.refresh();
        Ok(())
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn add_root(&mut self, path: PathBuf) -> Result<bool, String> {
        let root = path
            .canonicalize()
            .map_err(|error| format!("Could not open folder {}: {error}", path.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a folder", path.display()));
        }
        if self.roots.iter().any(|existing| {
            existing == &root || existing.canonicalize().is_ok_and(|path| path == root)
        }) {
            return Ok(false);
        }
        let mut roots = self.roots.clone();
        roots.push(root);
        self.save_roots(&roots)?;
        self.roots = roots;
        self.refresh();
        Ok(true)
    }

    pub fn remove_root(&mut self, path: &Path) -> Result<(), String> {
        let roots: Vec<_> = self
            .roots
            .iter()
            .filter(|root| root.as_path() != path)
            .cloned()
            .collect();
        if roots.len() != self.roots.len() {
            self.save_roots(&roots)?;
            self.roots = roots;
            self.refresh();
        }
        Ok(())
    }

    pub fn entries(&mut self, path: &Path) -> Result<&[FolderEntry], String> {
        match self
            .listings
            .entry(path.to_owned())
            .or_insert_with(|| read_entries(path))
        {
            Ok(entries) => Ok(entries.as_slice()),
            Err(error) => Err(error.clone()),
        }
    }

    pub fn refresh(&mut self) {
        self.listings.clear();
    }

    fn save_roots(&self, roots: &[PathBuf]) -> Result<(), String> {
        let Some(path) = &self.preferences_path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&Preferences {
            roots: roots.to_vec(),
        })
        .map_err(|error| format!("Could not encode Custom folders: {error}"))?;
        atomic_write(path, &bytes).map_err(|error| {
            format!(
                "Could not save Custom folders to {}: {error}",
                path.display()
            )
        })
    }
}

fn preferences_path() -> Result<PathBuf, String> {
    let nonempty_env = |key| std::env::var_os(key).filter(|value| !value.is_empty());
    #[cfg(target_os = "macos")]
    let root = nonempty_env("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/Sympoie"));
    #[cfg(target_os = "windows")]
    let root = nonempty_env("APPDATA").map(|home| PathBuf::from(home).join("Sympoie"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let root = nonempty_env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| nonempty_env("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|root| root.join("sympoie"));
    root.map(|root| root.join("library-folders.json"))
        .ok_or_else(|| "Could not locate the user settings folder for Custom folders".to_owned())
}

fn read_entries(path: &Path) -> Result<Vec<FolderEntry>, String> {
    let read = || -> io::Result<Vec<FolderEntry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let is_dir = file_type.is_dir();
            let is_file = file_type.is_file() || (file_type.is_symlink() && entry.path().is_file());
            if is_dir || is_file {
                entries.push(FolderEntry {
                    path: entry.path(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_dir,
                });
            }
        }
        entries.sort_by_cached_key(|entry| {
            (
                !entry.is_dir,
                entry.name.to_lowercase(),
                entry.name.clone(),
                entry.path.clone(),
            )
        });
        Ok(entries)
    };
    read().map_err(|error| format!("Could not read folder {}: {error}", path.display()))
}

pub fn is_audio_file(path: &Path) -> bool {
    sympoie::audio_import::has_supported_extension(path)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    for _ in 0..16 {
        let temporary = path.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            SAVE_SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let result = write_result.and_then(|()| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a temporary Custom folder preferences file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sympoie-library-folders-{}-{}",
                std::process::id(),
                SAVE_SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }

        fn folder(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preferences_reload_and_removal_preserve_the_bookmarked_files() {
        let temp = TempFolder::new();
        let root = temp.folder("Samples");
        let sample = root.join("kick.wav");
        fs::write(&sample, b"sample contents").unwrap();
        let path = temp.0.join("settings/library-folders.json");
        let mut folders = LibraryFolders::default();
        folders.load_preferences_from(path.clone()).unwrap();
        assert!(folders.add_root(root.clone()).unwrap());

        let mut reloaded = LibraryFolders::default();
        reloaded.load_preferences_from(path.clone()).unwrap();
        assert_eq!(reloaded.roots(), std::slice::from_ref(&root));
        reloaded.remove_root(&root).unwrap();
        assert_eq!(fs::read(&sample).unwrap(), b"sample contents");
        folders.load_preferences_from(path).unwrap();
        assert!(folders.roots().is_empty());
    }

    #[test]
    fn default_is_memory_only_and_duplicate_roots_are_ignored() {
        let temp = TempFolder::new();
        let root = temp.folder("Samples");
        let nested = temp.folder("Samples/nested");
        let mut folders = LibraryFolders::default();
        assert!(folders.preferences_path.is_none());
        assert!(folders.add_root(root.clone()).unwrap());
        assert!(!folders.add_root(nested.join("..")).unwrap());
        assert_eq!(folders.roots(), &[root]);
        assert!(folders.preferences_path.is_none());
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn unavailable_saved_roots_are_retained_and_errors_are_cached() {
        let temp = TempFolder::new();
        let missing = temp.0.join("disconnected-drive");
        let path = temp.0.join("folders.json");
        let preferences = Preferences {
            roots: vec![missing.clone(), missing.clone()],
        };
        fs::write(&path, serde_json::to_vec(&preferences).unwrap()).unwrap();
        let mut folders = LibraryFolders::default();
        folders.load_preferences_from(path).unwrap();
        assert_eq!(folders.roots(), std::slice::from_ref(&missing));
        assert!(folders.entries(&missing).is_err());
        fs::create_dir(&missing).unwrap();
        assert!(folders.entries(&missing).is_err());
        folders.refresh();
        assert!(folders.entries(&missing).unwrap().is_empty());
    }

    #[test]
    fn listings_sort_folders_first_and_change_only_after_refresh() {
        let temp = TempFolder::new();
        temp.folder("zebra");
        temp.folder("Alpha");
        fs::write(temp.0.join("z.wav"), []).unwrap();
        fs::write(temp.0.join("B.txt"), []).unwrap();
        fs::write(temp.0.join("a.WAVE"), []).unwrap();
        let mut folders = LibraryFolders::default();
        let entries = folders.entries(&temp.0).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "zebra", "a.WAVE", "B.txt", "z.wav"]
        );
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir);
        fs::remove_file(temp.0.join("z.wav")).unwrap();
        fs::write(temp.0.join("c.wav"), []).unwrap();
        assert_eq!(folders.entries(&temp.0).unwrap()[4].name, "z.wav");
        folders.refresh();
        assert_eq!(folders.entries(&temp.0).unwrap()[4].name, "c.wav");
    }

    #[test]
    fn invalid_paths_and_load_save_errors_are_reported() {
        let temp = TempFolder::new();
        let root = temp.folder("Samples");
        let path = temp.0.join("folders.json");
        fs::write(&path, b"invalid JSON").unwrap();
        let mut folders = LibraryFolders::default();
        assert!(folders.add_root(path.clone()).is_err());
        assert!(folders.add_root(temp.0.join("missing")).is_err());
        assert!(folders.load_preferences_from(path.clone()).is_err());
        assert!(folders.roots().is_empty());

        folders.preferences_path = Some(path.join("blocked.json"));
        assert!(folders.add_root(root.clone()).is_err());
        assert!(folders.roots().is_empty());
        folders.preferences_path = None;
        folders.add_root(root.clone()).unwrap();
        folders.preferences_path = Some(path.join("blocked.json"));
        assert!(folders.remove_root(&root).is_err());
        assert_eq!(folders.roots(), &[root]);
    }

    #[cfg(unix)]
    #[test]
    fn directory_links_cannot_create_cycles_and_file_links_are_visible() {
        use std::os::unix::fs::symlink;

        let temp = TempFolder::new();
        let root = temp.folder("Samples");
        fs::write(root.join("kick.wav"), []).unwrap();
        symlink(&root, root.join("loop")).unwrap();
        symlink(root.join("kick.wav"), root.join("link.wav")).unwrap();
        let mut folders = LibraryFolders::default();
        assert!(folders.add_root(root.clone()).unwrap());
        assert!(!folders.add_root(root.join("loop")).unwrap());
        let entries = folders.entries(&root).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| !entry.is_dir));
    }

    #[test]
    fn only_supported_audio_extensions_are_importable() {
        for path in [
            "kick.wav",
            "SNARE.WAV",
            "loop.WaVe",
            "kick.mp3",
            "loop.aiff",
            "pad.AIF",
            "loop.flac",
        ] {
            assert!(is_audio_file(Path::new(path)), "{path}");
        }
        for path in ["song.ogg", "loop.m4a", "wav", "kick.wav.txt"] {
            assert!(!is_audio_file(Path::new(path)), "{path}");
        }
    }
}
