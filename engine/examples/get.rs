//! Downloads one link from the command line, the way the app does, printing
//! progress twice a second:
//!
//! ```text
//! cargo run --release -p indir-engine --example get -- <link> [folder] [connections] [MB a second]
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use indir_engine::{client, CancellationToken, Limiter, Outcome, State, Transfer};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(link) = args.next() else {
        eprintln!("usage: get <link> [folder] [connections] [MB a second]");
        std::process::exit(2);
    };
    let folder = args.next().unwrap_or_else(|| ".".into());
    let connections = args.next().and_then(|n| n.parse().ok()).unwrap_or(8);
    let limiter = Limiter::new();
    limiter.set(args.next().and_then(|n| n.parse::<f64>().ok()).map(|mb| (mb * 1e6) as u64));

    let transfer = Arc::new(Transfer::new(State::new(link, folder.into())));
    let started = Instant::now();
    let watch = {
        let transfer = transfer.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let progress = transfer.progress();
                eprintln!(
                    "{:5.1} s  {:8.1} MB  {} connections",
                    started.elapsed().as_secs_f64(),
                    progress.received as f64 / 1e6,
                    progress.connections
                );
            }
        })
    };
    let outcome = transfer.run(&client("indir-example"), connections, &limiter, &CancellationToken::new()).await;
    watch.abort();
    match outcome {
        Outcome::Done(path) => println!("{} in {:.1} s", path.display(), started.elapsed().as_secs_f64()),
        other => {
            eprintln!("{other:?}");
            std::process::exit(1);
        }
    }
}
