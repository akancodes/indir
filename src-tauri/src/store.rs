//! Small JSON files in the app's data folder, written so that a crash
//! mid-write never leaves half a file behind.

use std::fs;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Reads a JSON file. One that doesn't parse is set aside as `.broken`
/// instead of being overwritten, and `None` comes back.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(e) => {
            eprintln!("indir: {} doesn't read as JSON ({e}); keeping it as .broken", path.display());
            let _ = fs::rename(path, path.with_extension("json.broken"));
            None
        }
    }
}

/// Writes the JSON beside the file first, then swaps it in.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&temporary, path)
}
