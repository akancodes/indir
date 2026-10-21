//! What the person chose: where downloads go, how many connections each one
//! gets, how many run at once and how fast they may go together.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub folder: PathBuf,
    #[serde(default = "default_connections")]
    pub connections: u8,
    /// How many downloads run at once; the others wait their turn. `None`:
    /// all of them.
    #[serde(default = "default_at_once")]
    pub at_once: Option<u8>,
    /// Bytes a second for every download together. `None`: no limit.
    #[serde(default)]
    pub speed_limit: Option<u64>,
}

fn default_connections() -> u8 {
    8
}

fn default_at_once() -> Option<u8> {
    Some(3)
}

impl Settings {
    pub fn load(path: &Path, downloads_folder: PathBuf) -> Self {
        let mut settings = store::read_json::<Settings>(path).unwrap_or(Settings {
            folder: PathBuf::new(),
            connections: default_connections(),
            at_once: default_at_once(),
            speed_limit: None,
        });
        if settings.folder.as_os_str().is_empty() {
            settings.folder = downloads_folder;
        }
        settings.connections = settings.connections.clamp(1, 16);
        settings.at_once = settings.at_once.map(|n| n.clamp(1, 16));
        settings.speed_limit = settings.speed_limit.filter(|&limit| limit > 0);
        settings
    }

    pub fn save(&self, path: &Path) {
        if let Err(e) = store::write_json(path, self) {
            eprintln!("indir: couldn't save the settings: {e}");
        }
    }
}
