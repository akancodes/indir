//! The list of downloads: starting, pausing and removing them, letting a
//! few run at a time, keeping the list on disk so it survives a restart, and
//! telling the window what changed. And what to do once everything is done.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use indir_engine::{CancellationToken, Client, Limiter, Outcome, State, Transfer};
use serde::{Deserialize, Serialize};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_notification::NotificationExt;

use crate::power::{self, Power};
use crate::settings::Settings;
use crate::store;

/// How often the window hears about progress.
const TICK: Duration = Duration::from_millis(500);
/// Ticks between saves of the list while something downloads.
const SAVE_EVERY: u32 = 6;
/// Seconds to change one's mind between the last download finishing and
/// the computer going to sleep, shutting down, or indir quitting.
const COUNTDOWN: u32 = 60;

pub struct Downloads {
    inner: Mutex<Inner>,
    client: Client,
    /// The speed limit every download shares.
    limiter: Limiter,
    power: Power,
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
    /// What happens once nothing is left to download. Never saved: it is
    /// for this time only.
    when_done: WhenDone,
    /// There while the countdown to `when_done` runs. Cancelling it stops it.
    countdown: Option<CancellationToken>,
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
    /// Waiting for its turn.
    Queued,
    Paused,
    Done,
    Failed,
}

/// What to do once every download is done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhenDone {
    #[default]
    Nothing,
    Sleep,
    ShutDown,
    Quit,
}

/// The countdown to what happens once every download is done.
#[derive(Clone, Serialize)]
struct Countdown {
    action: WhenDone,
    seconds: u32,
    /// Downloads on the list that stopped with a problem.
    failed: usize,
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
    /// The server has answered, so whether it can resume is known.
    answered: bool,
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
            status: Status::Queued,
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
            // Nothing starts by itself: what was running or waiting when indir
            // closed waits to be resumed.
            status: match record.status {
                Status::Downloading | Status::Queued => Status::Paused,
                status => status,
            },
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
            // Waiting comes back as paused anyway, and indir 0.1 can read that.
            status: if self.status == Status::Queued { Status::Paused } else { self.status },
            error: self.error.clone(),
            state: self.transfer.state(),
        }
    }

    /// Running, or waiting for its turn.
    fn active(&self) -> bool {
        !self.removing && (self.cancel.is_some() || self.status == Status::Queued)
    }

    fn name(&self) -> String {
        let name = self.transfer.progress().name;
        if name.is_empty() {
            indir_engine::link_name(&self.url)
        } else {
            name
        }
    }

    fn view(&self) -> View {
        let progress = self.transfer.progress();
        let answered = !progress.name.is_empty();
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
            answered,
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
        let limiter = Limiter::new();
        limiter.set(settings.speed_limit);
        Ok(Self {
            inner: Mutex::new(Inner {
                items,
                settings,
                next_id,
                changed: true,
                when_done: WhenDone::Nothing,
                countdown: None,
            }),
            client: indir_engine::client(&user_agent),
            limiter,
            power: Power::start(),
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

    /// Adds every web link in `text`, in the order given, and starts as many
    /// as may run at once. The others wait their turn.
    pub fn add(&self, app: &AppHandle, text: &str) -> Result<usize, String> {
        let links: Vec<String> = text.split_whitespace().filter_map(web_link).collect();
        if links.is_empty() {
            return Err("That isn't a link. Paste one that starts with http:// or https://.".into());
        }
        let (count, stopped) = {
            let mut inner = self.lock();
            let folder = inner.settings.folder.clone();
            let first = inner.next_id;
            inner.next_id += links.len() as u64;
            let added: Vec<Item> =
                links.into_iter().zip(first..).map(|(url, id)| Item::new(id, url, folder.clone())).collect();
            let count = added.len();
            inner.items.splice(0..0, added);
            inner.changed = true;
            (count, end_countdown(&mut inner))
        };
        countdown_ended(app, stopped);
        self.start_queued(app);
        self.save();
        Ok(count)
    }

    /// Starts waiting downloads, the earliest added first, while there's room.
    fn start_queued(&self, app: &AppHandle) {
        loop {
            let next = {
                let inner = self.lock();
                let running = inner.items.iter().filter(|item| item.cancel.is_some()).count();
                let room = inner.settings.at_once.is_none_or(|at_once| running < usize::from(at_once));
                let waiting = inner.items.iter().filter(|item| item.status == Status::Queued && !item.removing);
                if room {
                    waiting.map(|item| item.id).min()
                } else {
                    None
                }
            };
            match next {
                Some(id) if self.start(app, id) => {}
                _ => break,
            }
        }
    }

    /// Starts a download now, whatever is running. Returns whether it started.
    fn start(&self, app: &AppHandle, id: u64) -> bool {
        let (transfer, cancel, connections, stopped) = {
            let mut inner = self.lock();
            let connections = usize::from(inner.settings.connections);
            let Some(item) = inner.items.iter_mut().find(|item| item.id == id) else { return false };
            if item.cancel.is_some() || item.removing {
                return false;
            }
            let cancel = CancellationToken::new();
            item.cancel = Some(cancel.clone());
            item.status = Status::Downloading;
            item.error = None;
            item.meter = Meter::default();
            let transfer = item.transfer.clone();
            inner.changed = true;
            // Something to download again: the countdown waits for it. Under
            // the same lock, so it can't end in between.
            (transfer, cancel, connections, end_countdown(&mut inner))
        };
        countdown_ended(app, stopped);
        let (app, client, limiter) = (app.clone(), self.client.clone(), self.limiter.clone());
        tauri::async_runtime::spawn(async move {
            let outcome = transfer.run(&client, connections, &limiter, &cancel).await;
            app.state::<Downloads>().stopped(&app, id, outcome);
        });
        true
    }

    fn stopped(&self, app: &AppHandle, id: u64, outcome: Outcome) {
        let (ended, gone) = {
            let mut inner = self.lock();
            let Some(index) = inner.items.iter().position(|item| item.id == id) else { return };
            let mut ended = None;
            let removed = inner.items[index].removing;
            if removed {
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
                        ended = Some((item.name(), Ok(item.bytes)));
                    }
                    Outcome::Paused => item.status = Status::Paused,
                    Outcome::Failed(why) => {
                        item.status = Status::Failed;
                        item.error = Some(why.clone());
                        ended = Some((item.name(), Err(why)));
                    }
                }
            }
            inner.changed = true;
            // Finished, failed or removed, but not paused: perhaps nothing is left now.
            let gone = removed || ended.is_some();
            (ended, gone)
        };
        self.start_queued(app);
        if let Some((name, result)) = ended {
            notify(app, &name, result);
        }
        if gone {
            self.begin_countdown(app);
        }
        self.save();
    }

    /// Pauses a download, or takes one that waits out of the queue.
    pub fn pause(&self, id: u64) {
        {
            let mut inner = self.lock();
            let Some(item) = inner.items.iter_mut().find(|item| item.id == id) else { return };
            if let Some(cancel) = &item.cancel {
                // `stopped` marks it paused once its connections have closed.
                cancel.cancel();
                return;
            }
            if item.status != Status::Queued {
                return;
            }
            item.status = Status::Paused;
            inner.changed = true;
        }
        self.save();
    }

    /// Puts a paused or failed download back in the queue, which starts it
    /// at once if there's room.
    pub fn resume(&self, app: &AppHandle, id: u64) {
        let stopped = {
            let mut inner = self.lock();
            let stopped = |item: &&mut Item| item.id == id && matches!(item.status, Status::Paused | Status::Failed);
            let Some(item) = inner.items.iter_mut().find(stopped) else { return };
            item.status = Status::Queued;
            item.error = None;
            inner.changed = true;
            end_countdown(&mut inner)
        };
        countdown_ended(app, stopped);
        self.start_queued(app);
        self.save();
    }

    /// Starts a waiting download without waiting for its turn.
    pub fn start_now(&self, app: &AppHandle, id: u64) {
        let queued = self.lock().items.iter().any(|item| item.id == id && item.status == Status::Queued);
        if queued && self.start(app, id) {
            self.save();
        }
    }

    pub fn pause_all(&self) {
        {
            let mut inner = self.lock();
            for item in &mut inner.items {
                if let Some(cancel) = &item.cancel {
                    cancel.cancel();
                } else if item.status == Status::Queued {
                    item.status = Status::Paused;
                }
            }
            inner.changed = true;
        }
        self.save();
    }

    /// Puts every paused or failed download back in the queue, in the order
    /// they were added.
    pub fn resume_all(&self, app: &AppHandle) {
        let stopped = {
            let mut inner = self.lock();
            let mut any = false;
            for item in inner.items.iter_mut().filter(|item| matches!(item.status, Status::Paused | Status::Failed)) {
                item.status = Status::Queued;
                item.error = None;
                any = true;
            }
            inner.changed = true;
            any && end_countdown(&mut inner)
        };
        countdown_ended(app, stopped);
        self.start_queued(app);
        self.save();
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

    pub fn link(&self, id: u64) -> Option<String> {
        self.lock().items.iter().find(|item| item.id == id).map(|item| item.url.clone())
    }

    /// The links in copied `text` worth putting in the link field: only when
    /// the text is nothing but links, and only those not on the list already.
    /// Joined by spaces, as the field takes them.
    pub fn offer(&self, text: &str) -> Option<String> {
        // A whole copied page is no link.
        if text.len() > 16 << 10 {
            return None;
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let links: Vec<String> = words.iter().copied().filter_map(web_link).collect();
        if links.is_empty() || links.len() != words.len() {
            return None;
        }
        let inner = self.lock();
        let new: Vec<String> =
            links.into_iter().filter(|link| !inner.items.iter().any(|item| &item.url == link)).collect();
        (!new.is_empty()).then(|| new.join(" "))
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    pub fn set_connections(&self, connections: u8) -> Settings {
        self.change_settings(|settings| settings.connections = connections.clamp(1, 16))
    }

    /// How many downloads run at once, `None` for all. More room starts
    /// waiting ones; less lets the running ones finish.
    pub fn set_at_once(&self, app: &AppHandle, at_once: Option<u8>) -> Settings {
        let settings = self.change_settings(|settings| settings.at_once = at_once.map(|n| n.clamp(1, 16)));
        self.start_queued(app);
        settings
    }

    /// The speed limit for every download together, in bytes a second.
    /// Running downloads follow it at once.
    pub fn set_speed_limit(&self, limit: Option<u64>) -> Settings {
        let settings = self.change_settings(|settings| settings.speed_limit = limit.filter(|&limit| limit > 0));
        self.limiter.set(settings.speed_limit);
        settings
    }

    pub fn when_done(&self) -> WhenDone {
        self.lock().when_done
    }

    /// Chooses what happens once nothing is left to download. If nothing
    /// is left already, the countdown starts now.
    pub fn set_when_done(&self, app: &AppHandle, action: WhenDone) {
        self.lock().when_done = action;
        let _ = app.emit("when-done", action);
        if action == WhenDone::Nothing {
            self.stop_countdown(app, false);
            return;
        }
        self.begin_countdown(app);
        match action {
            WhenDone::ShutDown => {
                // macOS asks the person once whether indir may: better now than at the end.
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(why) = power::may_shut_down() {
                        let downloads = app.state::<Downloads>();
                        if downloads.when_done() == WhenDone::ShutDown {
                            downloads.set_when_done(&app, WhenDone::Nothing);
                        }
                        let _ = app.emit("problem", why);
                    }
                });
            }
            WhenDone::Nothing | WhenDone::Sleep | WhenDone::Quit => {}
        }
    }

    /// The countdown's Cancel: nothing happens after all.
    pub fn cancel_when_done(&self, app: &AppHandle) {
        self.stop_countdown(app, true);
    }

    /// The countdown's other button: does it now rather than in a minute.
    pub fn when_done_now(&self, app: &AppHandle) {
        let counting = self.lock().countdown.take();
        if let Some(countdown) = counting {
            countdown.cancel();
            self.act(app);
        }
    }

    /// Starts the countdown once nothing is left to download, if something
    /// should happen then. Brings the window forward, for anyone still there.
    fn begin_countdown(&self, app: &AppHandle) {
        let cancel = {
            let mut inner = self.lock();
            if inner.when_done == WhenDone::Nothing || inner.countdown.is_some() || inner.items.iter().any(Item::active)
            {
                return;
            }
            let cancel = CancellationToken::new();
            inner.countdown = Some(cancel.clone());
            cancel
        };
        crate::bring_forward(app);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let downloads = app.state::<Downloads>();
            for seconds in (1..=COUNTDOWN).rev() {
                let countdown = {
                    let inner = downloads.lock();
                    let failed = inner.items.iter().filter(|item| item.status == Status::Failed).count();
                    Countdown { action: inner.when_done, seconds, failed }
                };
                let _ = app.emit("countdown", Some(countdown));
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
            // Only this countdown's own end, and only if still nothing is left.
            let go = {
                let mut inner = downloads.lock();
                if cancel.is_cancelled() {
                    return;
                }
                inner.countdown = None;
                !inner.items.iter().any(Item::active)
            };
            if go {
                downloads.act(&app);
            } else {
                let _ = app.emit("countdown", None::<Countdown>);
            }
        });
    }

    /// Stops the countdown, if it runs. `forget` drops the choice as well.
    fn stop_countdown(&self, app: &AppHandle, forget: bool) {
        let (stopped, forgot) = {
            let mut inner = self.lock();
            let stopped = end_countdown(&mut inner);
            let forgot = forget && inner.when_done != WhenDone::Nothing;
            if forgot {
                inner.when_done = WhenDone::Nothing;
            }
            (stopped, forgot)
        };
        countdown_ended(app, stopped);
        if forgot {
            let _ = app.emit("when-done", WhenDone::Nothing);
        }
    }

    /// Does what was chosen, once. The choice is used up.
    fn act(&self, app: &AppHandle) {
        let action = std::mem::take(&mut self.lock().when_done);
        let _ = app.emit("countdown", None::<Countdown>);
        let _ = app.emit("when-done", WhenDone::Nothing);
        let (power_action, what) = match action {
            WhenDone::Nothing => return,
            WhenDone::Quit => {
                app.exit(0);
                return;
            }
            WhenDone::Sleep => (power::Action::Sleep, "put the computer to sleep"),
            WhenDone::ShutDown => (power::Action::ShutDown, "shut the computer down"),
        };
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(why) = app.state::<Downloads>().power.act(power_action) {
                let _ = app.emit("problem", format!("indir couldn't {what}: {why}"));
            }
        });
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
        // Held from the copy to the write, so a newer list is never written
        // over by an older one.
        let _saving = self.saving.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let records: Vec<Record> = self.lock().items.iter().filter(|item| !item.removing).map(Item::record).collect();
        if let Err(e) = store::write_json(&self.list_file, &records) {
            eprintln!("indir: couldn't save the list of downloads: {e}");
        }
    }

    /// Reads each running download's progress. Returns whether any is
    /// running, and whether something else changed since the last tick.
    /// Waiting ones don't count as running: nothing keeps the computer awake for them.
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
        let mut awake = false;
        loop {
            ticks.tick().await;
            let downloads = app.state::<Downloads>();
            let (running, changed) = downloads.tick();
            if running != awake {
                downloads.power.keep_awake(running);
                awake = running;
            }
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

/// Stops the countdown, if one runs, under the lock its end is decided
/// under. Returns whether one ran.
fn end_countdown(inner: &mut Inner) -> bool {
    match inner.countdown.take() {
        Some(countdown) => {
            countdown.cancel();
            true
        }
        None => false,
    }
}

/// Tells the window a countdown ended without anything happening.
fn countdown_ended(app: &AppHandle, ended: bool) {
    if ended {
        let _ = app.emit("countdown", None::<Countdown>);
    }
}

/// Says on the desktop that a download ended, unless indir's window is in
/// front anyway.
fn notify(app: &AppHandle, name: &str, result: Result<Option<u64>, String>) {
    let in_front = app.get_webview_window("main").and_then(|window| window.is_focused().ok()).unwrap_or(false);
    if in_front {
        return;
    }
    let body = match result {
        Ok(Some(bytes)) => format!("Downloaded, {}", size(bytes)),
        Ok(None) => "Downloaded".into(),
        Err(why) => format!("Stopped: {why}"),
    };
    if let Err(e) = app.notification().builder().title(name).body(body).show() {
        eprintln!("indir: couldn't show a notification: {e}");
    }
}

/// A size in words, the way the window writes it: 210 MB, 4.2 GB.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{} {}", value.round(), UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
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
    fn nothing_starts_by_itself_after_a_restart() {
        let record = |status| Record {
            id: 1,
            url: "https://example.com/a.zip".into(),
            added: 0,
            finished: None,
            bytes: None,
            status,
            error: None,
            state: State::new("https://example.com/a.zip".into(), PathBuf::from("/downloads")),
        };
        assert_eq!(Item::from_record(record(Status::Downloading)).status, Status::Paused);
        assert_eq!(Item::from_record(record(Status::Queued)).status, Status::Paused);
        assert_eq!(Item::from_record(record(Status::Done)).status, Status::Done);
    }

    #[test]
    fn writes_sizes_as_the_window_does() {
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1000), "1.0 KB");
        assert_eq!(size(210_000_000), "210 MB");
        assert_eq!(size(4_200_000_000), "4.2 GB");
        assert_eq!(size(999_960_000), "1.0 GB");
    }

    #[test]
    fn keeps_brackets_that_belong_to_the_link() {
        assert_eq!(
            web_link("https://en.wikipedia.org/wiki/Tea_(drink)").as_deref(),
            Some("https://en.wikipedia.org/wiki/Tea_(drink)")
        );
    }
}
