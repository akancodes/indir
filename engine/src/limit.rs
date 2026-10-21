//! A speed limit that every download shares.
//!
//! The limit is a budget of bytes a second. A connection asks for its share
//! before it keeps the bytes it has just received, and waits while the budget
//! is spent. While it waits it reads nothing, so the server slows down to
//! match. The budget may run a quarter of a second ahead, so a connection
//! isn't woken for every few kilobytes.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

/// How far ahead of the limit the bytes handed out may run.
const BURST: Duration = Duration::from_millis(250);

/// A speed limit for every [`Transfer`](crate::Transfer) run with a clone of
/// it, together. It starts with no limit. [`Limiter::set`] changes it, and
/// downloads already running follow at once.
#[derive(Clone, Default)]
pub struct Limiter {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    pace: Mutex<Pace>,
    /// Wakes the connections waiting for their turn when the limit changes.
    changed: Notify,
}

#[derive(Default)]
struct Pace {
    /// Bytes a second, or `None` for no limit.
    rate: Option<u64>,
    /// When the bytes handed out so far will have been paid for at `rate`.
    paid_until: Option<Instant>,
}

impl Limiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Limits the downloads that share this limiter to `rate` bytes a second
    /// between them, or lifts the limit with `None`. Zero means no limit too.
    pub fn set(&self, rate: Option<u64>) {
        {
            let mut pace = self.lock();
            pace.rate = rate.filter(|&rate| rate > 0);
            // The new limit counts from now.
            pace.paid_until = None;
        }
        self.shared.changed.notify_waiters();
    }

    /// The limit in bytes a second, if there is one.
    pub fn rate(&self) -> Option<u64> {
        self.lock().rate
    }

    /// Waits until `bytes` more fit within the limit.
    pub(crate) async fn take(&self, bytes: usize) {
        loop {
            // Asked for before the limit is read, so a change in between still wakes it.
            let changed = self.shared.changed.notified();
            let wait = {
                let mut pace = self.lock();
                let Some(rate) = pace.rate else { return };
                let now = Instant::now();
                let from = pace.paid_until.map_or(now, |until| until.max(now));
                let until = from + Duration::from_secs_f64(bytes as f64 / rate as f64);
                pace.paid_until = Some(until);
                until.saturating_duration_since(now).saturating_sub(BURST)
            };
            if wait.is_zero() {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => return,
                // Paid for at the old limit: ask again at the new one.
                () = changed => {}
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Pace> {
        self.shared.pace.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes `total` bytes in pieces of `piece`, the way a connection does.
    async fn take_all(limiter: &Limiter, total: usize, piece: usize) {
        for _ in 0..total / piece {
            limiter.take(piece).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_limit_never_waits() {
        let limiter = Limiter::new();
        let started = Instant::now();
        take_all(&limiter, 100 << 20, 1 << 20).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn keeps_to_the_limit() {
        let limiter = Limiter::new();
        limiter.set(Some(1_000_000));
        let started = Instant::now();
        take_all(&limiter, 3_000_000, 60_000).await;
        // Three seconds' worth, less the quarter second it may run ahead.
        let elapsed = started.elapsed();
        assert!(elapsed.abs_diff(Duration::from_millis(2750)) < Duration::from_millis(5), "{elapsed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn connections_share_the_limit() {
        let limiter = Limiter::new();
        limiter.set(Some(1_000_000));
        let started = Instant::now();
        let connections: Vec<_> = (0..4)
            .map(|_| {
                let limiter = limiter.clone();
                tokio::spawn(async move { take_all(&limiter, 500_000, 20_000).await })
            })
            .collect();
        for connection in connections {
            connection.await.unwrap();
        }
        // Four times half a megabyte at one megabyte a second, between them.
        let elapsed = started.elapsed();
        assert!(elapsed.abs_diff(Duration::from_millis(1750)) < Duration::from_millis(5), "{elapsed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_limit_wakes_waiting_connections() {
        let limiter = Limiter::new();
        limiter.set(Some(1000));
        let waiting = {
            let limiter = limiter.clone();
            // A megabyte at a kilobyte a second: over a quarter of an hour.
            tokio::spawn(async move { limiter.take(1_000_000).await })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!waiting.is_finished());
        limiter.set(None);
        tokio::time::timeout(Duration::from_millis(1), waiting).await.expect("lifting the limit lets it go").unwrap();
        assert_eq!(limiter.rate(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_higher_limit_shortens_the_wait() {
        let limiter = Limiter::new();
        limiter.set(Some(100_000));
        let started = Instant::now();
        let waiting = {
            let limiter = limiter.clone();
            // Ten seconds at the first limit.
            tokio::spawn(async move { limiter.take(1_000_000).await })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        limiter.set(Some(1_000_000));
        waiting.await.unwrap();
        // One second waited, then a fresh second at the new limit, less the burst.
        let elapsed = started.elapsed();
        assert!(elapsed.abs_diff(Duration::from_millis(1750)) < Duration::from_millis(5), "{elapsed:?}");
    }
}
