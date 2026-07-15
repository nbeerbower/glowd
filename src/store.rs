//! Persistence for user data. One pretty-printed JSON file in the state dir.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const PALETTE_FILE: &str = "colors.json";

/// `$GLOWD_STATE_DIR` if set, else `~/.local/state/glowd`.
pub fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GLOWD_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".local/state/glowd")
}

/// Missing file means an empty palette; a corrupt file is reported and skipped
/// rather than crashing the server.
pub fn load_palette(dir: &Path) -> Vec<String> {
    let path = dir.join(PALETTE_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("ignoring corrupt {}: {e}", path.display());
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Write-to-temp-then-rename so a crash mid-write can't corrupt the palette.
pub fn save_palette(dir: &Path, palette: &[String]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{PALETTE_FILE}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(palette)?)?;
    fs::rename(&tmp, dir.join(PALETTE_FILE))
}
