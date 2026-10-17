//! Watching the speed while a download's connections work.
//!
//! Some servers punish a visitor for opening several connections: a few get
//! full speed and the rest a trickle, and all of them together fetch less
//! than one connection would on its own. When the speeds show that pattern,
//! the download tries a single connection for a moment and keeps whichever
//! way was faster. Servers that treat every connection alike are left alone.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Time for connections to get up to speed before a measurement.
const SETTLE: Duration = Duration::from_secs(1);
/// How long a measurement lasts.
const WINDOW: Duration = Duration::from_millis(1500);
/// How much faster all the connections must be to win over one.
const GAIN: f64 = 1.1;
/// How many times faster the fastest connection must be than the slowest
/// for the server to count as punishing. Below that, a slower connection
/// still helps.
pub(crate) const CRAWLING: f64 = 16.0;

pub(crate) struct Steering {
    connections: usize,
    /// Bytes on disk before this run, so a resumed download's count isn't taken for speed.
    baseline: u64,
    phase: Phase,
    /// (when, bytes received by then), a few seconds' worth.
    samples: VecDeque<(Instant, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Every connection, until bytes flow and a fair measurement is possible.
    Many {
        flowing: Option<Instant>,
    },
    /// One connection on its own since `since`; `many` is what all of them managed.
    One {
        since: Instant,
        many: f64,
    },
    Settled,
}

/// What the download should do now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Carry,
    /// Go down to one connection: the one on this part.
    Narrow {
        keep: usize,
    },
    /// Go back to every connection.
    Widen,
}

impl Steering {
    pub fn new(connections: usize, baseline: u64) -> Self {
        let phase = if connections > 1 { Phase::Many { flowing: None } } else { Phase::Settled };
        Self { connections, baseline, phase, samples: VecDeque::new() }
    }

    pub fn connections(&self) -> usize {
        self.connections
    }

    /// Takes a reading: bytes received so far, the speed of each busy part
    /// that has one, and whether a connection has been seen crawling.
    pub fn step(&mut self, now: Instant, received: u64, speeds: &[(usize, f64)], crawled: bool) -> Decision {
        self.samples.push_back((now, received));
        while self.samples.front().is_some_and(|&(then, _)| now.duration_since(then) > 2 * WINDOW) {
            self.samples.pop_front();
        }
        match self.phase {
            Phase::Many { flowing: None } => {
                if received > self.baseline {
                    self.phase = Phase::Many { flowing: Some(now) };
                }
                Decision::Carry
            }
            Phase::Many { flowing: Some(since) } if now.duration_since(since) >= SETTLE + WINDOW => {
                let fastest = speeds.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
                let slowest = speeds.iter().map(|&(_, speed)| speed).fold(f64::INFINITY, f64::min);
                let spread = fastest.is_some_and(|(_, fast)| speeds.len() > 1 && slowest * CRAWLING < fast);
                if !(crawled || spread) {
                    self.phase = Phase::Settled;
                    return Decision::Carry;
                }
                // Punished, but no connection to keep yet: look again next time.
                let Some((keep, _)) = fastest else { return Decision::Carry };
                self.phase = Phase::One { since: now, many: self.rate() };
                Decision::Narrow { keep }
            }
            Phase::One { since, many } if now.duration_since(since) >= SETTLE + WINDOW => {
                self.phase = Phase::Settled;
                if many > self.rate() * GAIN {
                    Decision::Widen
                } else {
                    Decision::Carry
                }
            }
            _ => Decision::Carry,
        }
    }

    /// Bytes a second over the last measurement window.
    fn rate(&self) -> f64 {
        let Some(&(now, received)) = self.samples.back() else { return 0.0 };
        let Some(&(then, before)) = self.samples.iter().find(|&&(then, _)| now.duration_since(then) <= WINDOW) else {
            return 0.0;
        };
        let seconds = now.duration_since(then).as_secs_f64();
        if seconds > 0.0 {
            (received - before) as f64 / seconds
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(250);

    /// Feeds readings every 250 ms at `speed` bytes a second until a decision
    /// other than `Carry` comes, or `ticks` run out.
    fn feed(
        steering: &mut Steering,
        clock: &mut Instant,
        received: &mut u64,
        speed: u64,
        speeds: &[(usize, f64)],
        ticks: u32,
    ) -> Decision {
        for _ in 0..ticks {
            *clock += TICK;
            *received += speed / 4;
            let decision = steering.step(*clock, *received, speeds, false);
            if decision != Decision::Carry {
                return decision;
            }
        }
        Decision::Carry
    }

    #[test]
    fn leaves_alike_connections_alone() {
        let (mut clock, mut received) = (Instant::now(), 0);
        let mut steering = Steering::new(8, 0);
        let alike = [(0, 5e6), (1, 4e6), (2, 6e6)];
        assert_eq!(feed(&mut steering, &mut clock, &mut received, 15_000_000, &alike, 40), Decision::Carry);
        assert_eq!(steering.phase, Phase::Settled);
    }

    #[test]
    fn tries_one_connection_when_some_crawl_and_keeps_it_if_faster() {
        let (mut clock, mut received) = (Instant::now(), 0);
        let mut steering = Steering::new(8, 0);
        let crawling = [(0, 1e6), (3, 3e6), (5, 600.0)];
        let started = clock;
        assert_eq!(
            feed(&mut steering, &mut clock, &mut received, 4_000_000, &crawling, 40),
            Decision::Narrow { keep: 3 }
        );
        // Bytes flowed from the first reading on; then a settle and a window.
        assert_eq!(clock.duration_since(started), TICK + SETTLE + WINDOW);
        let alone = [(3, 8e6)];
        assert_eq!(feed(&mut steering, &mut clock, &mut received, 8_000_000, &alone, 40), Decision::Carry);
        assert_eq!(steering.phase, Phase::Settled);
    }

    #[test]
    fn goes_back_to_many_when_one_is_slower() {
        let (mut clock, mut received) = (Instant::now(), 0);
        let mut steering = Steering::new(8, 0);
        let crawling = [(0, 2e6), (1, 1000.0)];
        assert_eq!(
            feed(&mut steering, &mut clock, &mut received, 4_000_000, &crawling, 40),
            Decision::Narrow { keep: 0 }
        );
        assert_eq!(feed(&mut steering, &mut clock, &mut received, 2_000_000, &[(0, 2e6)], 40), Decision::Widen);
    }

    #[test]
    fn a_resumed_download_waits_for_new_bytes() {
        let mut steering = Steering::new(4, 1_000_000);
        let now = Instant::now();
        assert_eq!(steering.step(now, 1_000_000, &[], false), Decision::Carry);
        assert_eq!(steering.phase, Phase::Many { flowing: None });
    }

    #[test]
    fn one_connection_has_nothing_to_steer() {
        let mut steering = Steering::new(1, 0);
        assert_eq!(steering.phase, Phase::Settled);
        assert_eq!(steering.step(Instant::now(), 10, &[(0, 1.0), (1, 1e9)], true), Decision::Carry);
    }
}
