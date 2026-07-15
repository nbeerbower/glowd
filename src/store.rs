//! Persistence for user data. Pretty-printed JSON files in the state dir.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// `$GLOWD_STATE_DIR` if set, else `~/.local/state/glowd`.
pub fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GLOWD_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".local/state/glowd")
}

/// A missing file means "no data yet" (`T::default()`); a corrupt file is
/// reported and skipped rather than crashing the server.
pub fn load_json<T: DeserializeOwned + Default>(dir: &Path, file: &str) -> T {
    let path = dir.join(file);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("ignoring corrupt {}: {e}", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

/// Write-to-temp-then-rename so a crash mid-write can't corrupt the file.
pub fn save_json<T: Serialize>(dir: &Path, file: &str, value: &T) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{file}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, dir.join(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults_when_missing() {
        let dir = std::env::temp_dir().join(format!("glowd-store-test-{}", std::process::id()));
        save_json(&dir, "test.json", &vec!["#ff8800".to_string()]).unwrap();
        let loaded: Vec<String> = load_json(&dir, "test.json");
        assert_eq!(loaded, ["#ff8800"]);

        let missing: Vec<String> = load_json(&dir, "does-not-exist.json");
        assert!(missing.is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let dir = std::env::temp_dir().join(format!("glowd-corrupt-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.json"), "{not json").unwrap();
        let loaded: Vec<String> = load_json(&dir, "bad.json");
        assert!(loaded.is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
