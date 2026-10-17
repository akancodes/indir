//! The list of downloads: starting, pausing and removing them, keeping the
//! list on disk so it survives a restart, and telling the window what changed.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use indir_engine::{CancellationToken, Client, Outcome, State, Transfer};
use serde::{Deserialize, Serialize};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager, Url};

use crate::settings::Settings;
use crate::store;

/// How often the window hears about progress.
const TICK: Duration = Duration::from_millis(500);
/// Ticks between saves of the list while something downloads.
const SAVE_EVERY: u32 = 6;

pub struct Downloads {
    inner: Mutex<Inner>,
    client: Client,
    list_file: PathBuf,
    settings_file: PathBuf,
    /// Held while a file is written, so two saves never mix.
    saving: Mutex<()>,
}

struct Inner {
    /// Newest first.
    items: Vec<Item>,
    settings: Settings,
    next_id: u64,
    /// Something changed that the window hasn't heard about yet.
    changed: bool,
}

struct Item {
    id: u64,
    url: String,
    folder: PathBuf,
    added: u64,
    finished: Option<u64>,
    /// The finished file's size, which a download of unknown size only knows at the end.
    bytes: Option<u64>,
    status: Status,
    error: Option<String>,
    transfer: Arc<Transfer>,
    /// There while it runs. Cancelling it pauses the download.
    cancel: Option<CancellationToken>,
    /// Remove it, and its partial file, as soon as it has stopped.
    removing: bool,
    meter: Meter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Downloading,
    Paused,
    Done,
    Failed,
}

/// A download as kept in downloads.json.
#[derive(Serialize, Deserialize)]
struct Record {
    id: u64,
    url: String,
    added: u64,
    #[serde(default)]
    finished: Option<u64>,
    #[serde(default)]
    bytes: Option<u64>,
    status: Status,
    #[serde(default)]
    error: Option<String>,
    state: State,
}

/// A download as the window draws it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    id: u64,
    url: String,
    name: String,
    folder: String,
    status: Status,
    error: Option<String>,
    size: Option<u64>,
    received: u64,
    /// Bytes a second.
    speed: u64,
    /// Seconds left.
    eta: Option<u64>,
    resumable: bool,
    connections: usize,
    /// Each part as fractions of the file: start, end, and how far it has come.
    parts: Vec<[f64; 3]>,
    added: u64,
    finished: Option<u64>,
}

/// Where "Show in folder" goes.
pub enum Location {
    File(PathBuf),
    Folder(PathBuf),
}

/// The speed, smoothed over the last few readings.
#[derive(Default)]
struct Meter {
    last: Option<(Instant, u64)>,
    speed: f64,
}

impl Meter {
    fn read(&mut self, received: u64) {
        let now = Instant::now();
        if let Some((then, before)) = self.last {
            let seconds = now.duration_since(then).as_secs_f64();
            if seconds > 0.0 {
                let current = received.saturating_sub(before) as f64 / seconds;
                self.speed = if self.speed == 0.0 { current } else { self.speed * 0.7 + current * 0.3 };
            }
        }
        self.last = Some((now, received));
    }
}

impl Item {
    fn new(id: u64, url: String, folder: PathBuf) -> Self {
        let transfer = Arc::new(Transfer::new(State::new(url.clone(), folder.clone())));
        Self {
            id,
            url,
            folder,
            added: now(),
            finished: None,
            bytes: None,
            status: Status::Downloading,
            error: None,
            transfer,
            cancel: None,
            removing: false,
            meter: Meter::default(),
        }
    }

    fn from_record(record: Record) -> Self {
        Self {
            id: record.id,
            url: record.url,
            folder: record.state.dir.clone(),
            added: record.added,
            finished: record.finished,
            bytes: record.bytes,
            // A download that was running when indir closed waits to be resumed.
            status: if record.status == Status::Downloading { Status::Paused } else { record.status },
            error: record.error,
            transfer: Arc::new(Transfer::new(record.state)),
            cancel: None,
            removing: false,
            meter: Meter::default(),
        }
    }

    fn record(&self) -> Record {
        Record {
            id: self.id,
            url: self.url.clone(),
            added: self.added,
            finished: self.finished,
            bytes: self.bytes,
            status: self.status,
            error: self.error.clone(),
            state: self.transfer.state(),
        }
    }

    fn view(&self) -> View {
        let progress = self.transfer.progress();
        let downloading = self.status == Status::Downloading;
        let speed = if downloading { self.meter.speed as u64 } else { 0 };
        let size = progress.size.or(self.bytes);
        let eta = match size {
            Some(size) if downloading && speed > 0 => Some(size.saturating_sub(progress.received) / speed),
            _ => None,
        };
        let parts = match size {
            Some(size) if size > 0 && progress.resumable => {
                let total = size as f64;
                progress
                    .parts
                    .iter()
                    .map(|&(start, end, received)| {
                        [start as f64 / total, end as f64 / total, (start + received) as f64 / total]
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        View {
            id: self.id,
            url: self.url.clone(),
            name: if progress.name.is_empty() { indir_engine::link_name(&self.url) } else { progress.name },
            folder: self.folder.display().to_string(),
            status: self.status,
            error: self.error.clone(),
            size,
            received: if self.status == Status::Done { size.unwrap_or(progress.received) } else { progress.received },
            speed,
            eta,
            resumable: progress.resumable,
            connections: progress.connections,
            parts,
            added: self.added,
            finished: self.finished,
        }
    }
}

impl Downloads {
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data = app.path().app_data_dir()?;
        fs::create_dir_all(&data)?;
        let list_file = data.join("downloads.json");
        let settings_file = data.join("settings.json");
        let downloads_folder = app.path().download_dir().or_else(|_| app.path().home_dir())?;
        let settings = Settings::load(&settings_file, downloads_folder);
        let items: Vec<Item> = store::read_json::<Vec<Record>>(&list_file)
            .unwrap_or_default()
            .into_iter()
            .map(Item::from_record)
            .collect();
        let next_id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        let user_agent = format!("indir/{} (+https://github.com/akancodes/indir)", env!("CARGO_PKG_VERSION"));
        Ok(Self {
            inner: Mutex::new(Inner { items, settings, next_id, changed: true }),
            client: indir_engine::client(&user_agent),
            list_file,
            settings_file,
            saving: Mutex::new(()),
        })
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    pub fn views(&self) -> Vec<View> {
        self.lock().items.iter().filter(|item| !item.removing).map(Item::view).collect()
    }

    /// Adds every web link in `text`, in the order given, and starts them.
    pub fn add(&self, app: &AppHandle, text: &str) -> Result<usize, String> {
        let links: Vec<String> = text.split_whitespace().filter_map(web_link).collect();
        if links.is_empty() {
            return Err("That isn't a link. Paste one that starts with http:// or https://.".into());
        }
        let ids: Vec<u64> = {
            let mut inner = self.lock();
            let folder = inner.settings.folder.clone();
            let first = inner.next_id;
            inner.next_id += links.len() as u64;
            let added: Vec<Item> =
                links.into_iter().zip(first..).map(|(url, id)| Item::new(id, url, folder.clone())).collect();
            let ids = added.iter().map(|item| item.id).collect();
            inner.items.splice(0..0, added);
            ids
        };
        for &id in &ids {
            self.start(app, id);
        }
        self.save();
        Ok(ids.len())
    }

    fn start(&self, app: &AppHandle, id: u64) {
        let (transfer, cancel, connections) = {
            let mut inner = self.lock();
            let connections = usize::from(inner.settings.connections);
            let Some(item) = inner.items.iter_mut().find(|item| item.id == id) else { return };
            if item.cancel.is_some() || item.removing {
                return;
            }
            let cancel = CancellationToken::new();
            item.cancel = Some(cancel.clone());
            item.status = Status::Downloading;
            item.error = None;
            item.meter = Meter::default();
            let transfer = item.transfer.clone();
            inner.changed = true;
            (transfer, cancel, connections)
        };
        let (app, client) = (app.clone(), self.client.clone());
        tauri::async_runtime::spawn(async move {
            let outcome = transfer.run(&client, connections, &cancel).await;
            app.state::<Downloads>().stopped(id, outcome);
        });
    }

    fn stopped(&self, id: u64, outcome: Outcome) {
        {
            let mut inner = self.lock();
            let Some(index) = inner.items.iter().position(|item| item.id == id) else { return };
            if inner.items[index].removing {
                let item = inner.items.remove(index);
                remove_partial(&item.transfer.state());
            } else {
                let item = &mut inner.items[index];
                item.cancel = None;
                match outcome {
                    Outcome::Done(path) => {
                        item.status = Status::Done;
                        item.finished = Some(now());
                        item.bytes = fs::metadata(path).ok().map(|m| m.len());
                    }
                    Outcome::Paused => item.status = Status::Paused,
                    Outcome::Failed(why) => {
                        item.status = Status::Failed;
                        item.error = Some(why);
                    }
                }
            }
            inner.changed = true;
        }
        self.save();
    }

    pub fn pause(&self, id: u64) {
        if let Some(cancel) = self.lock().items.iter().find(|item| item.id == id).and_then(|item| item.cancel.as_ref())
        {
            cancel.cancel();
        }
    }

    pub fn resume(&self, app: &AppHandle, id: u64) {
        let stopped = self
            .lock()
            .items
            .iter()
            .any(|item| item.id == id && matches!(item.status, Status::Paused | Status::Failed));
        if stopped {
            self.start(app, id);
        }
    }

    /// Takes a download off the list. An unfinished one's partial file goes
    /// too; a finished file stays where it is.
    pub fn remove(&self, id: u64) {
        {
            let mut inner = self.lock();
            let Some(index) = inner.items.iter().position(|item| item.id == id) else { return };
            let item = &mut inner.items[index];
            if let Some(cancel) = &item.cancel {
                // Running: `stopped` removes it once its connections have closed.
                cancel.cancel();
                item.removing = true;
            } else {
                let item = inner.items.remove(index);
                if item.status != Status::Done {
                    remove_partial(&item.transfer.state());
                }
            }
            inner.changed = true;
        }
        self.save();
    }

    pub fn clear_finished(&self) {
        {
            let mut inner = self.lock();
            inner.items.retain(|item| item.status != Status::Done);
            inner.changed = true;
        }
        self.save();
    }

    pub fn finished_path(&self, id: u64) -> Result<PathBuf, String> {
        let inner = self.lock();
        let item = inner.items.iter().find(|item| item.id == id).ok_or("That download isn't on the list any more.")?;
        let path = item.transfer.state().path();
        if item.status == Status::Done && path.exists() {
            Ok(path)
        } else {
            Err("The file isn't there any more. It may have been moved or deleted.".into())
        }
    }

    pub fn location(&self, id: u64) -> Result<Location, String> {
        let inner = self.lock();
        let item = inner.items.iter().find(|item| item.id == id).ok_or("That download isn't on the list any more.")?;
        let state = item.transfer.state();
        let file = if item.status == Status::Done { state.path() } else { state.partial_path() };
        Ok(if !state.name.is_empty() && file.exists() {
            Location::File(file)
        } else {
            Location::Folder(item.folder.clone())
        })
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    pub fn set_connections(&self, connections: u8) -> Settings {
        self.change_settings(|settings| settings.connections = connections.clamp(1, 16))
    }

    pub fn set_folder(&self, folder: PathBuf) -> Settings {
        self.change_settings(|settings| settings.folder = folder)
    }

    fn change_settings(&self, change: impl FnOnce(&mut Settings)) -> Settings {
        let settings = {
            let mut inner = self.lock();
            change(&mut inner.settings);
            inner.changed = true;
            inner.settings.clone()
        };
        let _saving = self.saving.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        settings.save(&self.settings_file);
        settings
    }

    /// Writes the list to disk, counting only bytes that reached the disk,
    /// so it can always carry on from what it says.
    pub fn save(&self) {
        let records: Vec<Record> = self.lock().items.iter().filter(|item| !item.removing).map(Item::record).collect();
        let _saving = self.saving.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(e) = store::write_json(&self.list_file, &records) {
            eprintln!("indir: couldn't save the list of downloads: {e}");
        }
    }

    /// Reads each running download's progress. Returns whether any is
    /// running, and whether something else changed since the last tick.
    fn tick(&self) -> (bool, bool) {
        let mut inner = self.lock();
        let mut running = false;
        for item in inner.items.iter_mut().filter(|item| item.status == Status::Downloading) {
            item.meter.read(item.transfer.progress().received);
            running = true;
        }
        (running, std::mem::take(&mut inner.changed))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Every half second: progress to the window and the taskbar, and now and
/// then the list to disk.
pub fn watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(TICK);
        let mut beat = 0u32;
        let mut shown = Taskbar::Idle;
        loop {
            ticks.tick().await;
            let downloads = app.state::<Downloads>();
            let (running, changed) = downloads.tick();
            if running || changed {
                let views = downloads.views();
                let _ = app.emit("downloads", &views);
                let taskbar = Taskbar::of(&views);
                if taskbar != shown {
                    taskbar.show(&app);
                    shown = taskbar;
                }
            }
            beat = beat.wrapping_add(1);
            if running && beat % SAVE_EVERY == 0 {
                downloads.save();
            }
        }
    });
}

/// The progress bar on the app's taskbar button or Dock icon.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Taskbar {
    Idle,
    Percent(u64),
    Unknown,
}

impl Taskbar {
    fn of(views: &[View]) -> Self {
        let running: Vec<&View> = views.iter().filter(|view| view.status == Status::Downloading).collect();
        if running.is_empty() {
            return Self::Idle;
        }
        let mut received = 0;
        let mut size = 0;
        for view in running {
            let Some(view_size) = view.size else { return Self::Unknown };
            received += view.received;
            size += view_size;
        }
        Self::Percent((received * 100).checked_div(size).unwrap_or(0))
    }

    fn show(self, app: &AppHandle) {
        let Some(window) = app.get_webview_window("main") else { return };
        let (status, progress) = match self {
            Self::Idle => (ProgressBarStatus::None, None),
            Self::Percent(percent) => (ProgressBarStatus::Normal, Some(percent)),
            Self::Unknown => (ProgressBarStatus::Indeterminate, None),
        };
        let _ = window.set_progress_bar(ProgressBarState { status: Some(status), progress });
    }
}

/// A web link from a pasted word, without the brackets or quotes around it.
fn web_link(word: &str) -> Option<String> {
    let word = word.trim_end_matches([',', ';', '.']).trim_matches(['<', '>', '"', '\'']);
    let url = Url::parse(word).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn remove_partial(state: &State) {
    if !state.name.is_empty() {
        let _ = fs::remove_file(state.partial_path());
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_in_pasted_text() {
        let found: Vec<_> = "see <https://example.com/a.zip>, and \"http://example.com/b.iso\"; ftp://x/y not-a-link"
            .split_whitespace()
            .filter_map(web_link)
            .collect();
        assert_eq!(found, ["https://example.com/a.zip", "http://example.com/b.iso"]);
    }

    #[test]
    fn keeps_brackets_that_belong_to_the_link() {
        assert_eq!(
            web_link("https://en.wikipedia.org/wiki/Tea_(drink)").as_deref(),
            Some("https://en.wikipedia.org/wiki/Tea_(drink)")
        );
    }
}
