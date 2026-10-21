//! One download: planning its parts, fetching them over several connections,
//! and keeping count of every byte that reached the disk.

use std::fs;
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, IF_RANGE, RANGE};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::limit::Limiter;
use crate::name;
use crate::probe::{self, Probe};
use crate::steering::{Decision, Steering, CRAWLING};

/// Added to a file's name while it downloads: `movie.mkv.indir`.
pub const PARTIAL_EXTENSION: &str = "indir";

/// The smallest part a download starts with.
const MIN_PART: u64 = 1 << 20;
/// The least each half keeps when a free connection takes over half of a
/// part. Small enough that a slow connection near the end still gets help.
const MIN_SPLIT: u64 = 256 << 10;
/// Bytes gathered in memory before each write.
const WRITE_SIZE: usize = 256 << 10;
/// The most bytes a connection asks the speed limit for at once, so one
/// waiting its turn never waits long.
const LIMIT_SLICE: usize = 64 << 10;
/// How long a connection may go quiet before it is opened again.
const STALL: Duration = Duration::from_secs(30);
/// How long a connection may send nothing before a free connection takes
/// over what's left of its part. A server that is slow to answer gets four
/// times its first answer, if that's longer.
const QUIET: Duration = Duration::from_secs(5);
/// How often a free connection looks again for something to take.
const LOOK_AGAIN: Duration = Duration::from_millis(100);
/// How often the speeds are read while parts download.
const STEER_EVERY: Duration = Duration::from_millis(250);
/// How often what has been written is made durable, so `state` may count it.
const SYNC_EVERY: Duration = Duration::from_secs(2);
/// How long a finished file may stay locked (by a virus scanner, say) before
/// it can be given its name.
const RENAME_TRIES: u32 = 30;
/// Failed tries in a row, with no byte arriving, before a download gives up.
const MAX_TRIES: u32 = 6;
/// The most connections one download opens.
const MAX_CONNECTIONS: usize = 32;

/// Everything needed to carry on a download later. Store it as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The link as given.
    pub url: String,
    /// The folder the file goes to.
    pub dir: PathBuf,
    /// The file's name, chosen once the server has answered; empty before that.
    #[serde(default)]
    pub name: String,
    /// The file's size, if the server says it.
    #[serde(default)]
    pub size: Option<u64>,
    /// Whether the server sends byte ranges, so the file can come in parts and resume.
    #[serde(default)]
    pub resumable: bool,
    /// The server's ETag or Last-Modified, to notice the file changing between sessions.
    #[serde(default)]
    pub validator: Option<String>,
    /// The parts, and how much of each is on disk.
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// The bytes `start..end` of the file, of which the first `done` are on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub start: u64,
    pub end: u64,
    pub done: u64,
}

impl State {
    pub fn new(url: String, dir: PathBuf) -> Self {
        Self { url, dir, ..Self::default() }
    }

    /// Where the file sits while it downloads.
    pub fn partial_path(&self) -> PathBuf {
        partial_path(&self.dir, &self.name)
    }

    /// Where the file ends up.
    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The whole file is there, under this path.
    Done(PathBuf),
    /// Stopped on request. Run it again to carry on.
    Paused,
    /// Stopped by a problem, described for people. Running it again retries.
    Failed(String),
}

/// What a download looks like right now.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub name: String,
    pub size: Option<u64>,
    /// Bytes received so far, including any not written yet.
    pub received: u64,
    pub resumable: bool,
    /// Connections open right now.
    pub connections: usize,
    /// Each part as (start, end, bytes received).
    pub parts: Vec<(u64, u64, u64)>,
}

/// One download. Keep it in an [`Arc`]: [`Transfer::run`] fetches the file
/// while [`Transfer::progress`] and [`Transfer::state`] can be read from
/// anywhere.
pub struct Transfer {
    inner: Mutex<Inner>,
}

struct Inner {
    state: State,
    /// The state as of the last sync: every byte it counts is on disk for
    /// sure, not just in the system's cache. This is what `state()` returns.
    durable: State,
    /// The link as given, and the address the parts come from: where it
    /// redirected to, which may be a signed address that expires.
    link: Option<Url>,
    fetch_url: Option<Url>,
    /// Beside `state.parts`: what each part has received and whether a
    /// connection is on it.
    live: Vec<Live>,
    /// Bytes received by a download without ranges, which has no parts.
    streamed: u64,
    streaming: bool,
    /// How long a connection may stay quiet before its part is taken over.
    patience: Duration,
    /// How many connections may work at once right now (see `steering`).
    allowed: usize,
    /// Whether a connection was taken over for crawling along at a trickle.
    crawled: bool,
    /// Whether a speed limit is set. Connections are slow on purpose then.
    limited: bool,
}

struct Live {
    received: u64,
    busy: bool,
    /// When this part last received bytes, or last asked for them.
    heard: Instant,
    /// When the current request's first bytes came, and `received` just before.
    flowing: Option<(Instant, u64)>,
    /// Cancelled when another connection takes over what's left of the part.
    cut: CancellationToken,
    /// Waiting for the speed limit to let it go on: held back, not stuck.
    held: bool,
}

impl Live {
    fn new(received: u64) -> Self {
        Self { received, busy: false, heard: Instant::now(), flowing: None, cut: CancellationToken::new(), held: false }
    }

    /// Bytes a second since the current request's first bytes, once that is
    /// long enough ago to tell.
    fn speed(&self) -> Option<f64> {
        let (since, before) = self.flowing?;
        let seconds = since.elapsed().as_secs_f64();
        (seconds >= 1.0).then(|| (self.received - before) as f64 / seconds)
    }
}

/// Why fetching stopped, inside the engine.
#[derive(Debug)]
pub(crate) enum Stop {
    /// Asked to stop.
    Cancelled,
    /// Network trouble worth another try.
    Retry(String),
    /// The server is too busy for this request (429 or 503): fewer
    /// connections, or a wait, should do.
    Busy(String),
    /// A problem that trying again won't fix.
    Fatal(String),
}

/// How a connection stopped working on a part, short of a problem.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    Complete,
    /// Told to stop: a free connection took the rest over, or the download
    /// went down to fewer connections.
    Cut,
    /// The server said it was too busy, so this connection left its part to
    /// the others and closes.
    StepAside,
}

/// What a free connection should do next.
#[derive(Debug, PartialEq, Eq)]
enum Take {
    Part(usize),
    /// Nothing is worth taking yet, but parts are still on their way.
    Wait,
    Done,
}

impl Transfer {
    pub fn new(state: State) -> Self {
        let live = state.parts.iter().map(|p| Live::new(p.done)).collect();
        Self {
            inner: Mutex::new(Inner {
                durable: state.clone(),
                state,
                link: None,
                fetch_url: None,
                live,
                streamed: 0,
                streaming: false,
                patience: QUIET,
                allowed: MAX_CONNECTIONS,
                crawled: false,
                limited: false,
            }),
        }
    }

    /// A copy of the state, counting only bytes that are on disk for sure:
    /// save it to carry on after a restart, even one that wasn't planned.
    pub fn state(&self) -> State {
        self.lock().durable.clone()
    }

    pub fn progress(&self) -> Progress {
        let inner = self.lock();
        let state = &inner.state;
        let parts: Vec<_> = state.parts.iter().zip(&inner.live).map(|(p, l)| (p.start, p.end, l.received)).collect();
        Progress {
            name: state.name.clone(),
            size: state.size,
            received: if state.resumable { parts.iter().map(|p| p.2).sum() } else { inner.streamed },
            resumable: state.resumable,
            connections: inner.live.iter().filter(|l| l.busy).count() + usize::from(inner.streaming),
            parts,
        }
    }

    /// Fetches the file over up to `connections` connections, within the
    /// speed limit of `limiter`, until it is done, `cancel` fires (that's
    /// [`Outcome::Paused`]) or a problem stops it.
    pub async fn run(
        self: &Arc<Self>,
        client: &Client,
        connections: usize,
        limiter: &Limiter,
        cancel: &CancellationToken,
    ) -> Outcome {
        let result = self.fetch(client, connections.clamp(1, MAX_CONNECTIONS), limiter, cancel).await;
        {
            let mut inner = self.lock();
            inner.streaming = false;
            inner.live.iter_mut().for_each(|l| l.busy = false);
        }
        match result {
            Ok(path) => Outcome::Done(path),
            Err(Stop::Cancelled) => Outcome::Paused,
            Err(Stop::Retry(why) | Stop::Busy(why) | Stop::Fatal(why)) => Outcome::Failed(why),
        }
    }

    async fn fetch(
        self: &Arc<Self>,
        client: &Client,
        connections: usize,
        limiter: &Limiter,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, Stop> {
        let link = self.lock().state.url.clone();
        let url = Url::parse(&link)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .ok_or_else(|| Stop::Fatal("This isn't a web link".into()))?;
        let (probe, answered_in) = self.probe(client, &url, cancel).await?;
        {
            let mut inner = self.lock();
            inner.patience = QUIET.max(answered_in * 4);
            inner.link = Some(url);
            inner.fetch_url = Some(probe.url.clone());
        }
        self.prepare(&probe, connections)?;
        if self.lock().state.resumable {
            self.fetch_parts(client, connections, limiter, cancel).await?;
        } else {
            self.fetch_whole(client, &probe.url, limiter, cancel).await?;
        }
        self.finish().await
    }

    /// Asks the server about the file, trying again twice if it doesn't
    /// answer. Returns the answer and how long the answering try took.
    async fn probe(&self, client: &Client, url: &Url, cancel: &CancellationToken) -> Result<(Probe, Duration), Stop> {
        let mut tries = 0;
        loop {
            let asked = Instant::now();
            let result = tokio::select! {
                _ = cancel.cancelled() => return Err(Stop::Cancelled),
                result = probe::probe(client, url) => result,
            };
            match result {
                Ok(probe) => return Ok((probe, asked.elapsed())),
                Err(Stop::Retry(_) | Stop::Busy(_)) if tries < 2 => {
                    tries += 1;
                    wait(cancel, Duration::from_secs(tries)).await?;
                }
                Err(stop) => return Err(stop),
            }
        }
    }

    /// Gets the partial file ready: a new one under a free name, last time's
    /// if the server's file is unchanged, or a fresh start if it isn't.
    fn prepare(&self, probe: &Probe, connections: usize) -> Result<(), Stop> {
        let mut inner = self.lock();
        let Inner { state, live, streamed, .. } = &mut *inner;
        let resumable = probe.ranges && probe.size.is_some();
        fs::create_dir_all(&state.dir).map_err(|e| file_failure("create the folder", e))?;
        if state.name.is_empty() {
            let (name, file) = claim(&state.dir, &probe.name).map_err(|e| file_failure("create the file", e))?;
            state.name = name;
            if resumable {
                size_file(&file, probe.size.unwrap_or(0))?;
            }
            state.parts.clear();
        } else {
            // A validator that changed, or went missing, means a different file.
            let same_file = state.validator.is_none() || state.validator == probe.validator;
            let unchanged = state.resumable
                && resumable
                && state.size == probe.size
                && same_file
                && fs::metadata(state.partial_path()).is_ok_and(|m| Some(m.len()) == state.size);
            if !unchanged {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(state.partial_path())
                    .map_err(|e| file_failure("create the file", e))?;
                if resumable {
                    size_file(&file, probe.size.unwrap_or(0))?;
                }
                state.parts.clear();
            }
        }
        state.size = probe.size;
        state.resumable = resumable;
        state.validator.clone_from(&probe.validator);
        if resumable && state.parts.is_empty() {
            state.parts = plan(probe.size.unwrap_or(0), connections);
        }
        *live = state.parts.iter().map(|p| Live::new(p.done)).collect();
        *streamed = 0;
        // Done counts here are last session's durable ones, or zero.
        inner.durable = inner.state.clone();
        Ok(())
    }

    /// Runs the connections side by side until every part is on disk.
    async fn fetch_parts(
        self: &Arc<Self>,
        client: &Client,
        connections: usize,
        limiter: &Limiter,
        cancel: &CancellationToken,
    ) -> Result<(), Stop> {
        // A child token, so one connection's failure can stop the others
        // without it counting as a pause.
        let stop = cancel.child_token();
        let mut steering = {
            let mut inner = self.lock();
            inner.allowed = connections;
            inner.crawled = false;
            inner.limited = limiter.rate().is_some();
            Steering::new(connections, inner.live.iter().map(|l| l.received).sum())
        };
        let mut workers = JoinSet::new();
        for _ in 0..connections {
            let (me, client, limiter, stop) = (Arc::clone(self), client.clone(), limiter.clone(), stop.clone());
            workers.spawn(async move { me.connection(&client, &limiter, &stop).await });
        }
        let mut ticks = tokio::time::interval(STEER_EVERY);
        let mut last_sync = Instant::now();
        let mut syncing: Option<tokio::task::JoinHandle<()>> = None;
        let mut problem = None;
        loop {
            tokio::select! {
                joined = workers.join_next() => {
                    let Some(joined) = joined else { break };
                    let result = joined
                        .unwrap_or_else(|e| Err(Stop::Fatal(format!("A connection stopped unexpectedly: {e}"))));
                    if let Err(Stop::Retry(why) | Stop::Busy(why) | Stop::Fatal(why)) = result {
                        if problem.is_none() {
                            problem = Some(Stop::Fatal(why));
                            stop.cancel();
                        }
                    }
                }
                _ = ticks.tick() => {
                    self.follow_limit(limiter.rate().is_some());
                    self.steer(&mut steering);
                    // One sync at a time, in the background.
                    if syncing.as_ref().is_none_or(|sync| sync.is_finished()) && last_sync.elapsed() >= SYNC_EVERY {
                        let me = Arc::clone(self);
                        syncing = Some(tokio::spawn(async move { me.sync().await }));
                        last_sync = Instant::now();
                    }
                }
            }
        }
        // Every connection has stopped and written what it had: one last sync
        // makes all of it count, after any sync still running.
        if let Some(sync) = syncing {
            let _ = sync.await;
        }
        self.sync().await;
        if let Some(problem) = problem {
            return Err(problem);
        }
        if cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        if self.lock().state.parts.iter().any(|p| p.start + p.done < p.end) {
            return Err(Stop::Fatal("Some of the file didn't arrive".into()));
        }
        Ok(())
    }

    /// One connection: takes parts until every part is on its way or done.
    async fn connection(&self, client: &Client, limiter: &Limiter, stop: &CancellationToken) -> Result<(), Stop> {
        // How fast this connection fetched its last part, to compare with the others.
        let mut speed = None;
        loop {
            match self.take(speed) {
                Take::Part(i) => {
                    let (started, before) = (Instant::now(), self.lock().live[i].received);
                    let ended = self.fetch_part(client, i, limiter, stop).await;
                    // Stepping aside already let go of the part, under the lock;
                    // another connection may have it by now.
                    if let Ok(Ended::StepAside) = ended {
                        return Ok(());
                    }
                    let (after, left) = {
                        let mut inner = self.lock();
                        let Inner { state, live, .. } = &mut *inner;
                        live[i].busy = false;
                        (live[i].received, remaining(&state.parts[i], &live[i]))
                    };
                    // Taken over: this connection had gone quiet or slow, and
                    // the server may allow fewer fast connections than were
                    // opened, so it closes rather than look for more work.
                    if ended? == Ended::Cut && left == 0 {
                        return Ok(());
                    }
                    let seconds = started.elapsed().as_secs_f64();
                    if after > before && seconds > 0.0 {
                        speed = Some((after - before) as f64 / seconds);
                    }
                }
                Take::Wait => wait(stop, LOOK_AGAIN).await?,
                Take::Done => return Ok(()),
            }
        }
    }

    /// The next thing for a free connection to do, given how fast it fetched
    /// its last part: a part nobody is on; what's left of a part whose
    /// connection is stuck; or the second half of the biggest part still on
    /// its way.
    fn take(&self, speed: Option<f64>) -> Take {
        let mut inner = self.lock();
        let Inner { state, live, patience, allowed, crawled, limited, .. } = &mut *inner;
        // Room for one more connection. Taking over a stuck part needs none:
        // the stuck connection stops.
        let room = live.iter().filter(|l| l.busy).count() < *allowed;
        let pending = (0..live.len()).find(|&i| !live[i].busy && remaining(&state.parts[i], &live[i]) > 0);
        if let Some(i) = pending.filter(|_| room) {
            live[i].busy = true;
            live[i].heard = Instant::now();
            live[i].cut = CancellationToken::new();
            live[i].held = false;
            return Take::Part(i);
        }
        let running: Vec<(usize, u64)> = (0..live.len())
            .filter(|&i| live[i].busy)
            .map(|i| (i, remaining(&state.parts[i], &live[i])))
            .filter(|&(_, left)| left > 0)
            .collect();
        let Some(&(biggest, biggest_left)) = running.iter().max_by_key(|&&(_, left)| left) else {
            return if pending.is_some() { Take::Wait } else { Take::Done };
        };
        // Stuck parts, with how many seconds each would still need where it is.
        let stuck = running
            .iter()
            .filter_map(|&(i, left)| {
                if live[i].held {
                    return None;
                }
                if live[i].heard.elapsed() >= *patience {
                    return Some((i, left, f64::INFINITY));
                }
                // Under a speed limit every connection is slow on purpose, and
                // taking one over would fetch no sooner.
                if *limited {
                    return None;
                }
                let (mine, theirs) = (speed?, live[i].speed()?);
                let their_time = left as f64 / theirs.max(1.0);
                // Some servers give the first few connections full speed and
                // only a trickle to the rest.
                let crawling = theirs * CRAWLING < mine;
                *crawled |= crawling;
                // A last piece too small to split, at a pace this connection beats.
                let tail = left < 2 * MIN_SPLIT && their_time > 1.0 + 2.0 * left as f64 / mine;
                (crawling || tail).then_some((i, left, their_time))
            })
            .max_by(|a, b| a.2.total_cmp(&b.2));
        let (i, from) = match stuck {
            Some((i, left, _)) => {
                live[i].cut.cancel();
                (i, state.parts[i].end - left)
            }
            None if room && biggest_left >= 2 * MIN_SPLIT => (biggest, state.parts[biggest].end - biggest_left / 2),
            None => return Take::Wait,
        };
        // The connection on part i stops at `from` (see `accept`), and this
        // one takes the rest.
        let end = state.parts[i].end;
        state.parts[i].end = from;
        state.parts.push(Part { start: from, end, done: 0 });
        let mut taken = Live::new(0);
        taken.busy = true;
        live.push(taken);
        Take::Part(live.len() - 1)
    }

    /// Whether part `i`'s connection, told by the server that it's busy,
    /// should leave the part to the others: only while another connection is
    /// working and hasn't been told to stop. Decided under the lock, so the
    /// last connection always stays.
    fn step_aside(&self, i: usize) -> bool {
        let mut inner = self.lock();
        if inner.live.iter().enumerate().any(|(j, l)| j != i && l.busy && !l.cut.is_cancelled()) {
            inner.live[i].busy = false;
            true
        } else {
            false
        }
    }

    /// Notes whether a speed limit is set. Speeds measured under a limit say
    /// nothing about the connections once it is lifted, so they are measured
    /// afresh: otherwise the first to go fast would take the others for
    /// crawling and close them.
    fn follow_limit(&self, limited: bool) {
        let mut inner = self.lock();
        if inner.limited && !limited {
            inner.live.iter_mut().for_each(|l| l.flowing = None);
        }
        inner.limited = limited;
    }

    /// Makes what has been written durable, then lets `state()` count it.
    /// The copy is taken first, so everything it counts was written before
    /// the sync began, and is on disk once it returns.
    async fn sync(&self) {
        let (copy, path) = {
            let inner = self.lock();
            (inner.state.clone(), inner.state.partial_path())
        };
        let synced =
            tokio::task::spawn_blocking(move || fs::OpenOptions::new().write(true).open(path)?.sync_data()).await;
        if let Ok(Ok(())) = synced {
            self.lock().durable = copy;
        }
    }

    /// Reads the speeds and applies what the steering makes of them.
    fn steer(&self, steering: &mut Steering) {
        let mut inner = self.lock();
        let Inner { live, allowed, crawled, .. } = &mut *inner;
        let received = live.iter().map(|l| l.received).sum();
        let speeds: Vec<(usize, f64)> =
            live.iter().enumerate().filter(|(_, l)| l.busy).filter_map(|(i, l)| Some((i, l.speed()?))).collect();
        match steering.step(Instant::now(), received, &speeds, *crawled) {
            Decision::Carry => {}
            Decision::Narrow { keep } => {
                *allowed = 1;
                for (i, part) in live.iter().enumerate() {
                    if part.busy && i != keep {
                        part.cut.cancel();
                    }
                }
            }
            Decision::Widen => *allowed = steering.connections(),
        }
    }

    /// Fetches what's left of part `i`, opening the connection again after
    /// network trouble, waiting longer each time.
    async fn fetch_part(
        &self,
        client: &Client,
        i: usize,
        limiter: &Limiter,
        stop: &CancellationToken,
    ) -> Result<Ended, Stop> {
        let cut = self.lock().live[i].cut.clone();
        let mut tries = 0;
        loop {
            let before = self.lock().live[i].received;
            match self.fetch_range(client, i, limiter, stop, &cut).await {
                Err(Stop::Busy(_)) if self.step_aside(i) => return Ok(Ended::StepAside),
                Err(Stop::Retry(why) | Stop::Busy(why)) => {
                    if self.lock().live[i].received > before {
                        tries = 0;
                    }
                    tries += 1;
                    if tries >= MAX_TRIES {
                        return Err(Stop::Fatal(why));
                    }
                    tokio::select! {
                        _ = stop.cancelled() => return Err(Stop::Cancelled),
                        _ = cut.cancelled() => return Ok(Ended::Cut),
                        _ = tokio::time::sleep(Duration::from_secs(1 << tries.min(4))) => {}
                    }
                }
                result => return result,
            }
        }
    }

    /// One request for the rest of part `i`. Returns once the part is
    /// complete; whatever arrived before a problem is written first.
    async fn fetch_range(
        &self,
        client: &Client,
        i: usize,
        limiter: &Limiter,
        stop: &CancellationToken,
        cut: &CancellationToken,
    ) -> Result<Ended, Stop> {
        let (from, end, path, size, validator, link, fetch_url) = {
            let mut inner = self.lock();
            inner.live[i].heard = Instant::now();
            inner.live[i].flowing = None;
            let part = inner.state.parts[i];
            let state = &inner.state;
            (
                part.start + inner.live[i].received,
                part.end,
                state.partial_path(),
                state.size,
                // A weak ETag can't be used in If-Range: servers would never match it.
                state.validator.clone().filter(|v| !v.starts_with("W/")),
                inner.link.clone(),
                inner.fetch_url.clone(),
            )
        };
        if from >= end {
            return Ok(Ended::Complete);
        }
        let (Some(link), Some(fetch_url)) = (link, fetch_url) else {
            return Err(Stop::Fatal("The download has no address to fetch from".into()));
        };
        let range = (from, end, validator.as_deref());
        let Some(mut response) = ask(client, &fetch_url, range, stop, cut).await? else { return Ok(Ended::Cut) };
        if matches!(response.status().as_u16(), 401 | 403 | 410) && fetch_url != link {
            // Links that redirect often lead to a signed address that expires
            // after a while. Asking the link itself gets a fresh one.
            let Some(fresh) = ask(client, &link, range, stop, cut).await? else { return Ok(Ended::Cut) };
            response = fresh;
            if response.status().is_success() {
                self.lock().fetch_url = Some(response.url().clone());
            }
        }
        let changed = "The file on the server changed while it was downloading. Try again to start over.";
        match response.status() {
            StatusCode::PARTIAL_CONTENT => {
                let content_range = response.headers().get(CONTENT_RANGE).and_then(|v| v.to_str().ok());
                if content_range.and_then(probe::content_range_start) != Some(from) {
                    return Err(Stop::Fatal("The server sent the wrong part of the file".into()));
                }
                let total = content_range.and_then(probe::content_range_total);
                if total.is_some() && total != size {
                    return Err(Stop::Fatal(changed.into()));
                }
            }
            // With If-Range, a whole file instead of the part means it changed.
            StatusCode::OK if validator.is_some() => return Err(Stop::Fatal(changed.into())),
            StatusCode::OK if from == 0 => {}
            StatusCode::OK => {
                return Err(Stop::Fatal(
                    "The server stopped sending the file in parts. Remove it and add the link again.".into(),
                ))
            }
            status => return Err(http_failure(status)),
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(|e| file_failure("open the file", e))?;
        let mut pending = Vec::with_capacity(WRITE_SIZE);
        let mut body = response.bytes_stream();
        let result = 'reading: loop {
            let next = tokio::select! {
                _ = stop.cancelled() => break Err(Stop::Cancelled),
                _ = cut.cancelled() => break Ok(Ended::Cut),
                next = tokio::time::timeout(STALL, body.next()) => next,
            };
            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(e))) => break Err(Stop::Retry(describe(&e))),
                Ok(None) => break Err(Stop::Retry("The connection closed before its part was complete".into())),
                Err(_) => break Err(Stop::Retry("The server stopped sending data".into())),
            };
            for slice in chunk.chunks(LIMIT_SLICE) {
                if let Some(stopped) = self.wait_turn(i, slice.len(), limiter, stop, cut).await {
                    break 'reading stopped;
                }
                let (take, complete) = self.accept(i, slice.len());
                pending.extend_from_slice(&slice[..take]);
                if complete {
                    break 'reading Ok(Ended::Complete);
                }
                if pending.len() >= WRITE_SIZE {
                    if let Err(e) = self.write(&mut file, &mut pending, i).await {
                        break 'reading Err(e);
                    }
                }
            }
        };
        // Keep whatever arrived, however the request ended.
        self.write(&mut file, &mut pending, i).await?;
        result
    }

    /// Waits until the speed limit lets part `i` keep `len` more bytes, if
    /// there is a limit. Meanwhile the part counts as held back, not quiet.
    /// Returns how the part's request ends if it is stopped or cut first.
    async fn wait_turn(
        &self,
        i: usize,
        len: usize,
        limiter: &Limiter,
        stop: &CancellationToken,
        cut: &CancellationToken,
    ) -> Option<Result<Ended, Stop>> {
        limiter.rate()?;
        self.lock().live[i].held = true;
        let stopped = tokio::select! {
            _ = stop.cancelled() => Some(Err(Stop::Cancelled)),
            _ = cut.cancelled() => Some(Ok(Ended::Cut)),
            () = limiter.take(len) => None,
        };
        let mut inner = self.lock();
        inner.live[i].held = false;
        // The quiet that counts is the server's, from here on.
        inner.live[i].heard = Instant::now();
        stopped
    }

    /// Counts `len` new bytes for part `i`, cut at the part's end (which a
    /// connection taking its second half may have moved). Returns how many
    /// to keep and whether the part is now complete.
    fn accept(&self, i: usize, len: usize) -> (usize, bool) {
        let mut inner = self.lock();
        let Inner { state, live, .. } = &mut *inner;
        let part = state.parts[i];
        let take = remaining(&part, &live[i]).min(len as u64);
        if take > 0 {
            let now = Instant::now();
            live[i].heard = now;
            let received = live[i].received;
            live[i].flowing.get_or_insert((now, received));
        }
        live[i].received += take;
        (take as usize, remaining(&part, &live[i]) == 0)
    }

    /// Writes part `i`'s pending bytes where its done bytes end. They count as
    /// done once the file has them, and as durable after the next sync.
    /// Seeking every time makes a retry after a failed, partial write safe.
    async fn write(&self, file: &mut tokio::fs::File, pending: &mut Vec<u8>, i: usize) -> Result<(), Stop> {
        if pending.is_empty() {
            return Ok(());
        }
        let at = {
            let inner = self.lock();
            inner.state.parts[i].start + inner.state.parts[i].done
        };
        file.seek(SeekFrom::Start(at)).await.map_err(|e| file_failure("write the file", e))?;
        let written = write_out(file, pending).await?;
        self.lock().state.parts[i].done += written;
        Ok(())
    }

    /// A server without ranges sends the file in one go, from the start.
    async fn fetch_whole(
        &self,
        client: &Client,
        url: &Url,
        limiter: &Limiter,
        cancel: &CancellationToken,
    ) -> Result<(), Stop> {
        let (path, size) = {
            let inner = self.lock();
            (inner.state.partial_path(), inner.state.size)
        };
        // Only the wait for an answer is timed: a request timeout in reqwest
        // would also cover the body, and cut off any download longer than it.
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(Stop::Cancelled),
            response = tokio::time::timeout(STALL, client.get(url.clone()).send()) => match response {
                Ok(response) => response.map_err(|e| Stop::Fatal(describe(&e)))?,
                Err(_) => return Err(Stop::Fatal("The server stopped answering".into())),
            },
        };
        if !response.status().is_success() {
            return Err(http_failure(response.status()));
        }
        let mut file = tokio::fs::File::create(&path).await.map_err(|e| file_failure("create the file", e))?;
        self.lock().streaming = true;
        let mut pending = Vec::with_capacity(WRITE_SIZE);
        let mut body = response.bytes_stream();
        let result = 'reading: loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => break Err(Stop::Cancelled),
                next = tokio::time::timeout(STALL, body.next()) => next,
            };
            match next {
                Ok(Some(Ok(chunk))) => {
                    for slice in chunk.chunks(LIMIT_SLICE) {
                        tokio::select! {
                            _ = cancel.cancelled() => break 'reading Err(Stop::Cancelled),
                            () = limiter.take(slice.len()) => {}
                        }
                        self.lock().streamed += slice.len() as u64;
                        pending.extend_from_slice(slice);
                        if pending.len() >= WRITE_SIZE {
                            if let Err(e) = write_out(&mut file, &mut pending).await {
                                break 'reading Err(e);
                            }
                        }
                    }
                }
                Ok(None) => break Ok(()),
                Ok(Some(Err(e))) => break Err(Stop::Fatal(describe(&e))),
                Err(_) => break Err(Stop::Fatal("The server stopped sending data".into())),
            }
        };
        write_out(&mut file, &mut pending).await?;
        result?;
        if size.is_some_and(|size| self.lock().streamed < size) {
            return Err(Stop::Fatal("The connection closed before the file was complete".into()));
        }
        Ok(())
    }

    /// Gives the finished file its real name, or the next free one if that
    /// name was taken in the meantime.
    async fn finish(&self) -> Result<PathBuf, Stop> {
        let (dir, wanted, partial) = {
            let inner = self.lock();
            (inner.state.dir.clone(), inner.state.name.clone(), inner.state.partial_path())
        };
        let mut name = wanted.clone();
        let mut n = 0;
        while dir.join(&name).exists() {
            n += 1;
            name = name::numbered(&wanted, n);
        }
        let mut tries = 0;
        loop {
            match fs::rename(&partial, dir.join(&name)) {
                Ok(()) => break,
                // On Windows a virus scanner or the search indexer may hold a
                // file that was just written, for a moment.
                Err(e) if tries < RENAME_TRIES && e.kind() != io::ErrorKind::NotFound => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => return Err(file_failure("finish the file", e)),
            }
        }
        let mut inner = self.lock();
        inner.state.name = name;
        inner.durable = inner.state.clone();
        Ok(inner.state.path())
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn remaining(part: &Part, live: &Live) -> u64 {
    part.end.saturating_sub(part.start + live.received)
}

/// Splits `size` bytes into up to `connections` equal parts of at least `MIN_PART`.
fn plan(size: u64, connections: usize) -> Vec<Part> {
    if size == 0 {
        return Vec::new();
    }
    let count = (size / MIN_PART).clamp(1, connections as u64);
    let step = size / count;
    (0..count)
        .map(|i| Part { start: i * step, end: if i + 1 == count { size } else { (i + 1) * step }, done: 0 })
        .collect()
}

fn partial_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{PARTIAL_EXTENSION}"))
}

/// Finds a name that's free in `dir`, for the file and its partial file
/// alike, and creates the partial file. Creating it is the reservation, so
/// two downloads of the same link never pick the same name.
fn claim(dir: &Path, wanted: &str) -> io::Result<(String, fs::File)> {
    for n in 0..10_000 {
        let name = if n == 0 { wanted.to_string() } else { name::numbered(wanted, n) };
        if dir.join(&name).exists() {
            continue;
        }
        match fs::OpenOptions::new().read(true).write(true).create_new(true).open(partial_path(dir, &name)) {
            Ok(file) => return Ok((name, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "every name like this one is taken"))
}

/// Gives the partial file its final size up front. On Windows it is marked
/// sparse first, or NTFS would fill everything before a write near the end
/// with zeros.
fn size_file(file: &fs::File, size: u64) -> Result<(), Stop> {
    #[cfg(windows)]
    make_sparse(file);
    file.set_len(size).map_err(|e| file_failure("make room for the file", e))
}

#[cfg(windows)]
fn make_sparse(file: &fs::File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let mut returned = 0u32;
    // Best effort: FAT drives can't, and the download works either way.
    // SAFETY: the handle is open for writing for the whole call, and this
    // control code takes no buffers.
    unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

/// Asks for the bytes `from..end` of the file at `url`, with an If-Range
/// validator if there is one. `None` means the part was cut first.
async fn ask(
    client: &Client,
    url: &Url,
    (from, end, validator): (u64, u64, Option<&str>),
    stop: &CancellationToken,
    cut: &CancellationToken,
) -> Result<Option<reqwest::Response>, Stop> {
    let mut request = client.get(url.clone()).header(RANGE, format!("bytes={from}-{}", end - 1));
    if let Some(validator) = validator {
        request = request.header(IF_RANGE, validator);
    }
    tokio::select! {
        _ = stop.cancelled() => Err(Stop::Cancelled),
        _ = cut.cancelled() => Ok(None),
        response = tokio::time::timeout(STALL, request.send()) => match response {
            Ok(response) => response.map(Some).map_err(|e| Stop::Retry(describe(&e))),
            Err(_) => Err(Stop::Retry("The server stopped answering".into())),
        },
    }
}

/// Writes and flushes `pending`, and returns how many bytes that was.
async fn write_out(file: &mut tokio::fs::File, pending: &mut Vec<u8>) -> Result<u64, Stop> {
    if pending.is_empty() {
        return Ok(0);
    }
    file.write_all(pending).await.map_err(|e| file_failure("write the file", e))?;
    file.flush().await.map_err(|e| file_failure("write the file", e))?;
    let written = pending.len() as u64;
    pending.clear();
    Ok(written)
}

async fn wait(cancel: &CancellationToken, time: Duration) -> Result<(), Stop> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Stop::Cancelled),
        _ = tokio::time::sleep(time) => Ok(()),
    }
}

/// A network error in words. The innermost cause is the readable one.
pub(crate) fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "The server took too long to answer".into();
    }
    if error.is_redirect() {
        return "The link redirects too many times".into();
    }
    let mut cause: &dyn std::error::Error = error;
    while let Some(next) = cause.source() {
        cause = next;
    }
    if error.is_connect() {
        format!("Couldn't connect to the server: {cause}")
    } else {
        format!("The connection failed: {cause}")
    }
}

/// An HTTP status that isn't a success, in words. Trouble on the server's
/// side is worth another try; the rest isn't.
pub(crate) fn http_failure(status: StatusCode) -> Stop {
    let code = status.as_u16();
    let reason = status.canonical_reason().unwrap_or("error");
    let text = match code {
        401 | 403 => format!(
            "The server refused the download ({code} {reason}). The link may need you to be signed in, or it has expired."
        ),
        404 | 410 => format!("There's no file at this link ({code} {reason})"),
        _ => format!("The server answered {code} {reason}"),
    };
    match code {
        429 | 503 => Stop::Busy(text),
        408 | 500..=599 => Stop::Retry(text),
        _ => Stop::Fatal(text),
    }
}

fn file_failure(action: &str, error: io::Error) -> Stop {
    Stop::Fatal(format!("Couldn't {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_equal_parts_of_at_least_a_megabyte() {
        assert_eq!(plan(0, 8), vec![]);
        assert_eq!(plan(1000, 8), vec![Part { start: 0, end: 1000, done: 0 }]);
        let parts = plan(5 * MIN_PART + 3, 8);
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].start, 0);
        assert_eq!(parts[4].end, 5 * MIN_PART + 3);
        assert!(parts.windows(2).all(|w| w[0].end == w[1].start));
        assert_eq!(plan(100 * MIN_PART, 8).len(), 8);
    }

    #[test]
    fn a_free_connection_takes_half_of_the_biggest_part() {
        let size = 10 * MIN_PART;
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(size, 2);
        let transfer = Transfer::new(state);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(1, MIN_PART as usize);
        // Part 0 has 5 MB left, part 1 has 4 MB: the third connection splits part 0.
        assert_eq!(transfer.take(None), Take::Part(2));
        let parts = transfer.lock().state.parts.clone();
        assert_eq!(parts[0], Part { start: 0, end: 5 * MIN_PART / 2, done: 0 });
        assert_eq!(parts[2], Part { start: 5 * MIN_PART / 2, end: 5 * MIN_PART, done: 0 });
        // Part 0 now stops at its new end.
        assert_eq!(transfer.accept(0, size as usize), ((5 * MIN_PART / 2) as usize, true));
    }

    #[test]
    fn small_parts_are_not_split() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(MIN_SPLIT + MIN_SPLIT / 2, 4);
        let transfer = Transfer::new(state);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Wait);
        transfer.accept(0, (MIN_SPLIT + MIN_SPLIT / 2) as usize);
        assert_eq!(transfer.take(None), Take::Done);
    }

    #[test]
    fn a_crawling_part_is_taken_over_whole() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(4 * MIN_PART, 2);
        let transfer = Transfer::new(state);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(1, 2 * MIN_PART as usize);
        transfer.accept(0, 1000);
        // 1000 bytes in two seconds, against a connection doing 10 MB a second.
        transfer.lock().live[0].flowing = Some((Instant::now() - Duration::from_secs(2), 0));
        assert_eq!(transfer.take(Some(10e6)), Take::Part(2));
        assert_eq!(transfer.lock().state.parts.clone()[2], Part { start: 1000, end: 2 * MIN_PART, done: 0 });
        assert!(transfer.lock().live[0].cut.is_cancelled());
    }

    #[test]
    fn a_slower_part_that_still_helps_is_only_split() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(8 * MIN_PART, 2);
        let transfer = Transfer::new(state);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(1, 4 * MIN_PART as usize);
        transfer.accept(0, 2_000_000);
        // 1 MB a second against 10: slower, but worth keeping.
        transfer.lock().live[0].flowing = Some((Instant::now() - Duration::from_secs(2), 0));
        assert_eq!(transfer.take(Some(10e6)), Take::Part(2));
        assert!(!transfer.lock().live[0].cut.is_cancelled());
        let left = 4 * MIN_PART - 2_000_000;
        assert_eq!(transfer.lock().state.parts.clone()[0].end, 4 * MIN_PART - left / 2);
    }

    #[test]
    fn a_quiet_part_is_taken_over_whole() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(2 * MIN_PART, 2);
        let transfer = Transfer::new(state);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(1, MIN_PART as usize);
        transfer.accept(0, 1000);
        let cut = transfer.lock().live[0].cut.clone();
        transfer.lock().live[0].heard -= QUIET;
        assert_eq!(transfer.take(None), Take::Part(2));
        assert!(cut.is_cancelled());
        let parts = transfer.lock().state.parts.clone();
        assert_eq!(parts[0].end, 1000);
        assert_eq!(parts[2], Part { start: 1000, end: MIN_PART, done: 0 });
    }

    #[test]
    fn a_part_held_back_by_the_speed_limit_is_not_quiet() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        // Too small to split, so a free connection can only take a stuck part over.
        state.parts =
            vec![Part { start: 0, end: MIN_SPLIT, done: 0 }, Part { start: MIN_SPLIT, end: 2 * MIN_SPLIT, done: 0 }];
        let transfer = Transfer::new(state);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(0, 1000);
        transfer.accept(1, MIN_SPLIT as usize);
        transfer.lock().live[0].heard -= QUIET;
        transfer.lock().live[0].held = true;
        assert_eq!(transfer.take(None), Take::Wait);
        transfer.lock().live[0].held = false;
        assert_eq!(transfer.take(None), Take::Part(2));
    }

    #[test]
    fn speeds_are_measured_afresh_once_a_limit_is_lifted() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(4 * MIN_PART, 2);
        let transfer = Transfer::new(state);
        transfer.follow_limit(true);
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(1, 2 * MIN_PART as usize);
        transfer.accept(0, 1000);
        // Slow all along, under the limit.
        transfer.lock().live[0].flowing = Some((Instant::now() - Duration::from_secs(60), 0));
        transfer.follow_limit(false);
        assert_eq!(transfer.lock().live[0].flowing, None);
        // A fast connection doesn't take it for crawling: nothing is known about it yet.
        assert_eq!(transfer.take(Some(10e6)), Take::Part(2));
        assert!(!transfer.lock().live[0].cut.is_cancelled());
    }

    #[test]
    fn under_a_speed_limit_a_slow_part_is_only_split() {
        let mut state = State::new("https://example.com/f".into(), PathBuf::new());
        state.parts = plan(4 * MIN_PART, 2);
        let transfer = Transfer::new(state);
        transfer.lock().limited = true;
        assert_eq!(transfer.take(None), Take::Part(0));
        assert_eq!(transfer.take(None), Take::Part(1));
        transfer.accept(1, 2 * MIN_PART as usize);
        transfer.accept(0, 1000);
        // Slow enough to count as crawling without a limit.
        transfer.lock().live[0].flowing = Some((Instant::now() - Duration::from_secs(2), 0));
        assert_eq!(transfer.take(Some(10e6)), Take::Part(2));
        assert!(!transfer.lock().live[0].cut.is_cancelled());
        let left = 2 * MIN_PART - 1000;
        assert_eq!(transfer.lock().state.parts.clone()[0].end, 2 * MIN_PART - left / 2);
    }
}
