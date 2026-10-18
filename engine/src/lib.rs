//! The download engine behind [indir](https://github.com/akancodes/indir).
//!
//! A download is one file split into parts. Each connection asks the server
//! for one part's bytes (an HTTP `Range` request) and writes them at their
//! place in the file. A connection that finishes early takes over half of the
//! biggest part still on its way, so every connection stays busy until the
//! end. Progress is kept per part, so a paused, failed or interrupted download
//! carries on where each part stopped.
//!
//! Every download run with the same [`Limiter`] shares its speed limit.
//!
//! ```no_run
//! use std::sync::Arc;
//! use indir_engine::{client, CancellationToken, Limiter, Outcome, State, Transfer};
//!
//! # async fn example() {
//! let state = State::new("https://example.com/big.iso".into(), "/home/me/Downloads".into());
//! let transfer = Arc::new(Transfer::new(state));
//! let limiter = Limiter::new();
//! match transfer.run(&client("my-app/1.0"), 8, &limiter, &CancellationToken::new()).await {
//!     Outcome::Done(path) => println!("saved to {}", path.display()),
//!     Outcome::Paused => println!("paused: keep transfer.state() to carry on later"),
//!     Outcome::Failed(why) => println!("failed: {why}"),
//! }
//! # }
//! ```

mod limit;
mod name;
mod probe;
mod steering;
mod transfer;

use std::time::Duration;

pub use limit::Limiter;
pub use name::{link_name, sanitize};
pub use reqwest::Client;
pub use tokio_util::sync::CancellationToken;
pub use transfer::{Outcome, Part, Progress, State, Transfer, PARTIAL_EXTENSION};

/// An HTTP client set up for downloading. It speaks HTTP/1.1 only, so every
/// part gets a TCP connection of its own: HTTP/2 would carry them all over
/// one, and a single connection is exactly what a download manager avoids.
pub fn client(user_agent: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(20))
        .http1_only()
        .build()
        .expect("the HTTP client's settings are valid")
}
