//! Downloads from a small local server that can be slow, forget ranges, cut
//! connections or change the file, each checked byte for byte.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, State as Shared};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::stream;
use indir_engine::{client, CancellationToken, Outcome, State, Transfer};

#[derive(Clone)]
struct Server {
    /// The file and its ETag; tests may swap both.
    file: Arc<Mutex<(Arc<Vec<u8>>, String)>>,
    ranges: bool,
    length: bool,
    /// Each connection sends `chunk` bytes, then waits `delay`.
    chunk: usize,
    delay: Duration,
    /// Requests from offset 0 wait this many times longer.
    slow_start: u32,
    /// The first ranged request after offset 0 is cut after this many bytes.
    cut_after: Option<usize>,
    /// The first request from each of these offsets gets its headers, then nothing.
    silent_once: Arc<Mutex<Vec<u64>>>,
    /// The first request from each of these offsets gets a trickle: 1 KB every 50 ms.
    trickle_once: Arc<Mutex<Vec<u64>>>,
    /// Like some speed-test servers: only this many connections at a time get
    /// the bandwidth, which they share out, and the rest get a trickle.
    fast_slots: Option<usize>,
    /// Connections sending right now, and how many of them are fast.
    streams: Arc<Mutex<(usize, usize)>>,
    /// Answers 429 Too Many Requests to any request while this many are already sending.
    busy_over: Option<usize>,
    /// Like GitHub's release downloads: `…/redirect/<name>` sends each request to
    /// a new `…/signed/<n>/<name>`, and each of those works for two requests only.
    signed: Arc<Mutex<(u64, HashMap<u64, u32>)>>,
    disposition: Option<String>,
    cut: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
    requests: Arc<AtomicU64>,
}

impl Server {
    fn new(data: &[u8]) -> Self {
        Self {
            file: Arc::new(Mutex::new((Arc::new(data.to_vec()), "\"v1\"".into()))),
            ranges: true,
            length: true,
            chunk: 64 << 10,
            delay: Duration::ZERO,
            slow_start: 1,
            cut_after: None,
            silent_once: Arc::default(),
            trickle_once: Arc::default(),
            fast_slots: None,
            streams: Arc::default(),
            busy_over: None,
            signed: Arc::default(),
            disposition: None,
            cut: Arc::default(),
            sent: Arc::default(),
            requests: Arc::default(),
        }
    }

    /// Serves on a free port and returns the link to `name`.
    async fn start(&self, name: &str) -> String {
        let app = Router::new().route("/{*path}", get(serve)).with_state(self.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/files/{name}")
    }
}

async fn serve(Shared(server): Shared<Server>, UrlPath(path): UrlPath<String>, headers: HeaderMap) -> Response {
    server.requests.fetch_add(1, SeqCst);
    if path.ends_with("missing") {
        return Response::builder().status(404).body(Body::empty()).unwrap();
    }
    if path.contains("/redirect/") {
        let token = {
            let mut signed = server.signed.lock().unwrap();
            signed.0 += 1;
            signed.0
        };
        let to = format!("/{}", path.replace("/redirect/", &format!("/signed/{token}/")));
        return Response::builder().status(302).header(header::LOCATION, to).body(Body::empty()).unwrap();
    }
    if let Some((_, rest)) = path.split_once("/signed/") {
        let token: u64 = rest.split('/').next().unwrap().parse().unwrap();
        let mut signed = server.signed.lock().unwrap();
        let uses = signed.1.entry(token).or_default();
        *uses += 1;
        if *uses > 2 {
            return Response::builder().status(403).body(Body::empty()).unwrap();
        }
    }
    if server.busy_over.is_some_and(|limit| server.streams.lock().unwrap().0 >= limit) {
        return Response::builder().status(429).body(Body::empty()).unwrap();
    }
    let (data, etag) = server.file.lock().unwrap().clone();
    let total = data.len() as u64;
    let mut response = Response::builder().header(header::ETAG, &etag);
    if server.ranges {
        response = response.header(header::ACCEPT_RANGES, "bytes");
    }
    if let Some(disposition) = &server.disposition {
        response = response.header(header::CONTENT_DISPOSITION, disposition);
    }
    // If-Range: the range only while the file is still the one the client means.
    let same_file = headers.get(header::IF_RANGE).is_none_or(|validator| validator.to_str().unwrap() == etag);
    let asked = headers
        .get(header::RANGE)
        .filter(|_| server.ranges && same_file)
        .map(|value| parse_range(value.to_str().unwrap(), total));
    let (start, end) = match asked {
        None => {
            response = response.status(200);
            (0, total)
        }
        Some(Some((start, end))) => {
            response = response.status(206).header(header::CONTENT_RANGE, format!("bytes {start}-{}/{total}", end - 1));
            (start, end)
        }
        Some(None) => {
            return response
                .status(416)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(Body::empty())
                .unwrap()
        }
    };
    if server.length {
        response = response.header(header::CONTENT_LENGTH, end - start);
    }
    let delay = if start == 0 { server.delay * server.slow_start } else { server.delay };
    let cut_after = server.cut_after.filter(|_| start > 0 && !server.cut.swap(true, SeqCst));
    let once = |offsets: &Mutex<Vec<u64>>| {
        let mut offsets = offsets.lock().unwrap();
        let found = offsets.iter().position(|&offset| offset == start);
        found.map(|at| offsets.remove(at)).is_some()
    };
    let silent = once(&server.silent_once);
    let (chunk, delay) =
        if once(&server.trickle_once) { (1 << 10, Duration::from_millis(50)) } else { (server.chunk, delay) };
    let sent = server.sent.clone();
    let stream = Stream::open(&server);
    let (start, end) = (start as usize, end as usize);
    let body = stream::unfold((start, stream), move |(at, stream)| {
        let (data, sent) = (data.clone(), sent.clone());
        async move {
            if at >= end {
                return None;
            }
            if cut_after.is_some_and(|cut| at - start >= cut) {
                return Some((Err(std::io::Error::other("cut on purpose")), (end, stream)));
            }
            if silent {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            let (chunk, delay) = stream.pace(chunk, delay);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let next = (at + chunk).min(end);
            sent.fetch_add((next - at) as u64, SeqCst);
            Some((Ok(Bytes::copy_from_slice(&data[at..next])), (next, stream)))
        }
    });
    response.body(Body::from_stream(body)).unwrap()
}

/// One response body being sent, counted while it lasts.
struct Stream {
    streams: Arc<Mutex<(usize, usize)>>,
    /// Under `fast_slots`: whether this one got a fast slot.
    fast: Option<bool>,
}

impl Stream {
    fn open(server: &Server) -> Self {
        let mut counts = server.streams.lock().unwrap();
        counts.0 += 1;
        let fast = server.fast_slots.map(|slots| counts.1 < slots);
        if fast == Some(true) {
            counts.1 += 1;
        }
        Self { streams: server.streams.clone(), fast }
    }

    /// The chunk size and wait for the next chunk.
    fn pace(&self, chunk: usize, delay: Duration) -> (usize, Duration) {
        match self.fast {
            None => (chunk, delay),
            Some(false) => (1 << 10, Duration::from_millis(100)),
            Some(true) => {
                // The more connections a visitor opens, the slower each fast one gets.
                let open = self.streams.lock().unwrap().0 as u32;
                (chunk, delay * open * open)
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let mut counts = self.streams.lock().unwrap();
        counts.0 -= 1;
        if self.fast == Some(true) {
            counts.1 -= 1;
        }
    }
}

/// "bytes=a-b" or "bytes=a-" → `Some(start, end)` when it fits the file.
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let (from, to) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = from.parse().ok()?;
    let end = if to.is_empty() { total } else { (to.parse::<u64>().ok()? + 1).min(total) };
    (start < end).then_some((start, end))
}

/// Bytes that are different everywhere, so a part in the wrong place shows.
fn sample(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn new_transfer(url: &str, dir: &Path) -> Arc<Transfer> {
    Arc::new(Transfer::new(State::new(url.into(), dir.into())))
}

async fn run(transfer: &Arc<Transfer>, connections: usize) -> Outcome {
    transfer.run(&client("indir-tests"), connections, &CancellationToken::new()).await
}

/// Runs a download in the background and pauses it once bytes are flowing.
async fn run_then_pause(transfer: &Arc<Transfer>) -> Outcome {
    let cancel = CancellationToken::new();
    let task = {
        let (transfer, cancel) = (transfer.clone(), cancel.clone());
        tokio::spawn(async move { transfer.run(&client("indir-tests"), 4, &cancel).await })
    };
    until(|| transfer.progress().received > 0).await;
    cancel.cancel();
    task.await.unwrap()
}

/// Waits up to 10 seconds for `condition`.
async fn until(condition: impl Fn() -> bool) {
    for _ in 0..1000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("waited 10 seconds for something that didn't happen");
}

fn done_bytes(state: &State) -> u64 {
    state.parts.iter().map(|p| p.done).sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetches_a_file_in_parts() {
    let data = sample(6 << 20, 1);
    let server = Server::new(&data);
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    let state = transfer.state();
    assert!(state.resumable);
    assert_eq!(state.size, Some(data.len() as u64));
    assert!(state.parts.len() >= 4, "{:?}", state.parts);
    assert!(!dir.path().join("sample.bin.indir").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uses_one_connection_when_the_server_has_no_ranges() {
    let data = sample(3 << 20, 2);
    let server = Server { ranges: false, ..Server::new(&data) };
    let url = server.start("plain.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 8).await, Outcome::Done(dir.path().join("plain.bin")));
    assert!(std::fs::read(dir.path().join("plain.bin")).unwrap() == data);
    assert!(!transfer.state().resumable);
    // The question about the file, then the file itself.
    assert_eq!(server.requests.load(SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetches_a_file_of_unknown_size() {
    let data = sample(2 << 20, 3);
    let server = Server { ranges: false, length: false, ..Server::new(&data) };
    let url = server.start("stream.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 8).await, Outcome::Done(dir.path().join("stream.bin")));
    assert!(std::fs::read(dir.path().join("stream.bin")).unwrap() == data);
    assert_eq!(transfer.state().size, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pauses_and_carries_on_after_a_restart() {
    let data = sample(4 << 20, 4);
    let server = Server { chunk: 16 << 10, delay: Duration::from_millis(10), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run_then_pause(&transfer).await, Outcome::Paused);
    let state = transfer.state();
    let done = done_bytes(&state);
    assert!(done > 0 && done < data.len() as u64, "{done} of {}", data.len());
    assert!(dir.path().join("sample.bin.indir").exists());
    // A new Transfer from the saved state, as after quitting the app.
    let again = Arc::new(Transfer::new(state));
    assert_eq!(run(&again, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    // Nothing was fetched twice, beyond what was in flight at the pause.
    let sent = server.sent.load(SeqCst);
    assert!(sent < data.len() as u64 + (1 << 20), "sent {sent} for {}", data.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn carries_on_after_a_crash() {
    // Slow enough that a sync happens long before the end.
    let data = sample(6 << 20, 5);
    let server = Server { chunk: 16 << 10, delay: Duration::from_millis(40), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let task = {
        let transfer = transfer.clone();
        tokio::spawn(async move { run(&transfer, 4).await })
    };
    // The app saves the state now and then, and may die right after. What the
    // state counts must already be on disk.
    until(|| done_bytes(&transfer.state()) > 0).await;
    let saved = transfer.state();
    task.abort();
    let _ = task.await;
    let again = Arc::new(Transfer::new(saved));
    assert_eq!(run(&again, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tries_again_after_a_cut_connection() {
    let data = sample(4 << 20, 6);
    let server = Server { chunk: 32 << 10, cut_after: Some(300 << 10), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    assert!(server.cut.load(SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn free_connections_take_over_a_slow_part() {
    let data = sample(8 << 20, 7);
    let server = Server { chunk: 64 << 10, delay: Duration::from_millis(20), slow_start: 10, ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let started = Instant::now();
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    let parts = transfer.state().parts;
    assert!(parts.len() > 4, "the slow part was never split: {parts:?}");
    // Alone, the slow part's 2 MB at 0.3 MB a second would take over 6 seconds.
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn takes_over_connections_that_send_nothing() {
    // Servers that limit connections per visitor often accept the extra
    // ones and then send them nothing.
    let data = sample(8 << 20, 13);
    let server = Server { silent_once: Arc::new(Mutex::new(vec![2 << 20, 4 << 20])), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let started = Instant::now();
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    // Waiting for the quiet connections to time out would take over 30 seconds.
    assert!(started.elapsed() < Duration::from_secs(12), "{:?}", started.elapsed());
    assert!(server.silent_once.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn takes_over_connections_that_only_trickle() {
    // Other servers keep the extra connections at a trickle instead.
    let data = sample(8 << 20, 14);
    let server = Server { trickle_once: Arc::new(Mutex::new(vec![2 << 20, 4 << 20])), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let started = Instant::now();
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    // At 20 KB a second, the trickled parts alone would take minutes.
    assert!(started.elapsed() < Duration::from_secs(6), "{:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settles_on_one_connection_when_the_server_punishes_more() {
    let data = sample(16 << 20, 15);
    let server =
        Server { chunk: 128 << 10, delay: Duration::from_millis(20), fast_slots: Some(2), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let started = Instant::now();
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    // One connection alone gets 6.5 MB a second here. Four get 0.8 between
    // them, so staying with four would take over 15 seconds.
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opens_fewer_connections_when_the_server_says_too_many() {
    let data = sample(6 << 20, 16);
    let server = Server { chunk: 64 << 10, delay: Duration::from_millis(20), busy_over: Some(2), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let started = Instant::now();
    assert_eq!(run(&transfer, 8).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    // Two connections at 3.2 MB a second each. Retrying the refused ones
    // with growing waits instead would take far longer.
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starts_over_when_the_file_changed() {
    let data = sample(4 << 20, 8);
    let server = Server { chunk: 16 << 10, delay: Duration::from_millis(10), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run_then_pause(&transfer).await, Outcome::Paused);
    let changed = sample(4 << 20, 9);
    *server.file.lock().unwrap() = (Arc::new(changed.clone()), "\"v2\"".into());
    let again = Arc::new(Transfer::new(transfer.state()));
    assert_eq!(run(&again, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == changed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_mixes_two_versions_of_a_file() {
    // The first part is slow, so the others take it over after the file changed.
    let data = sample(8 << 20, 17);
    let server = Server { chunk: 64 << 10, delay: Duration::from_millis(20), slow_start: 10, ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let task = {
        let transfer = transfer.clone();
        tokio::spawn(async move { run(&transfer, 4).await })
    };
    until(|| transfer.progress().received > 0).await;
    let changed = sample(8 << 20, 18);
    *server.file.lock().unwrap() = (Arc::new(changed.clone()), "\"v2\"".into());
    let Outcome::Failed(why) = task.await.unwrap() else { panic!("a file that changed midway can't finish") };
    assert!(why.contains("changed"), "{why}");
    // Trying again starts over, with the new file only.
    let again = Arc::new(Transfer::new(transfer.state()));
    assert_eq!(run(&again, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == changed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asks_the_link_again_when_its_signed_address_expires() {
    let data = sample(4 << 20, 19);
    let server = Server::new(&data);
    let url = server.start("redirect/sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("sample.bin")));
    assert!(std::fs::read(dir.path().join("sample.bin")).unwrap() == data);
    // Each signed address works twice, so the link had to be asked again.
    assert!(server.signed.lock().unwrap().0 > 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explains_a_missing_file() {
    let server = Server::new(b"");
    let url = server.start("missing").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    let Outcome::Failed(why) = run(&transfer, 4).await else { panic!("a missing file can't download") };
    assert!(why.contains("404"), "{why}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "it left a file behind");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetches_an_empty_file() {
    let server = Server::new(b"");
    let url = server.start("empty.txt").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("empty.txt")));
    assert_eq!(std::fs::metadata(dir.path().join("empty.txt")).unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn takes_the_name_the_server_gives() {
    let data = sample(1000, 10);
    let server =
        Server { disposition: Some("attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf".into()), ..Server::new(&data) };
    let url = server.start("download").await;
    let dir = tempfile::tempdir().unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("résumé.pdf")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_link_twice_gets_two_names() {
    let data = sample(3 << 20, 11);
    let server = Server { chunk: 32 << 10, delay: Duration::from_millis(1), ..Server::new(&data) };
    let url = server.start("sample.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (new_transfer(&url, dir.path()), new_transfer(&url, dir.path()));
    let (a, b) = tokio::join!(run(&first, 4), run(&second, 4));
    let mut paths = [a, b].map(|outcome| match outcome {
        Outcome::Done(path) => path,
        other => panic!("{other:?}"),
    });
    paths.sort();
    assert_eq!(paths, [dir.path().join("sample (1).bin"), dir.path().join("sample.bin")]);
    for path in paths {
        assert!(std::fs::read(path).unwrap() == data);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_overwrites_a_file_that_is_already_there() {
    let data = sample(1000, 12);
    let server = Server::new(&data);
    let url = server.start("notes.txt").await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "mine").unwrap();
    let transfer = new_transfer(&url, dir.path());
    assert_eq!(run(&transfer, 4).await, Outcome::Done(dir.path().join("notes (1).txt")));
    assert_eq!(std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(), "mine");
}
