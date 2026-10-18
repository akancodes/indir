//! The system clipboard, for text only.

use std::sync::{Mutex, MutexGuard};

/// Opened on first use and kept for the app's whole life: on Linux, a link
/// indir copies is handed out from here until something else is copied.
/// Never use it on the main thread: on Linux, reading may wait on this very
/// app's window.
#[derive(Default)]
pub struct Clipboard(Mutex<Option<arboard::Clipboard>>);

impl Clipboard {
    pub fn read(&self) -> Result<String, String> {
        Self::open(&mut self.lock())?.get_text().map_err(|e| e.to_string())
    }

    pub fn write(&self, text: String) -> Result<(), String> {
        Self::open(&mut self.lock())?.set_text(text).map_err(|e| e.to_string())
    }

    fn open(clipboard: &mut Option<arboard::Clipboard>) -> Result<&mut arboard::Clipboard, String> {
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        Ok(clipboard.as_mut().expect("opened just now"))
    }

    fn lock(&self) -> MutexGuard<'_, Option<arboard::Clipboard>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
