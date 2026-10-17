//! What the person chose: where downloads go, and how many connections each one gets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub folder: PathBuf,
    #[serde(default = "default_connections")]
    pub connections: u8,
}

fn default_connections() -> u8 {
    8
}

impl Settings {
    pub fn load(path: &Path, downloads_folder: PathBuf) -> Self {
        let mut settings = store::read_json::<Settings>(path)
            .unwrap_or(Settings { folder: PathBuf::new(), connections: default_connections() });
        if settings.folder.as_os_str().is_empty() {
            settings.folder = downloads_folder;
        }
        settings.connections = settings.connections.clamp(1, 16);
        settings
    }

    pub fn save(&self, path: &Path) {
        if let Err(e) = store::write_json(path, self) {
            eprintln!("indir: couldn't save the settings: {e}");
        }
    }
}
