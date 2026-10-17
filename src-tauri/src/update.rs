//! One quiet question to GitHub per launch: is there a newer release?

use std::time::Duration;

use indir_engine::Client;
use serde::Serialize;

const LATEST: &str = "https://api.github.com/repos/akancodes/indir/releases/latest";

#[derive(Serialize)]
pub struct Update {
    pub version: String,
}

pub async fn check(client: &Client) -> Option<Update> {
    let response = client
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let release: serde_json::Value = serde_json::from_slice(&response.bytes().await.ok()?).ok()?;
    let latest = release.get("tag_name")?.as_str()?.trim_start_matches('v');
    newer(latest, env!("CARGO_PKG_VERSION")).then(|| Update { version: latest.to_string() })
}

/// Whether version `a` comes after `b`, comparing "1.2.3" number by number.
fn newer(a: &str, b: &str) -> bool {
    let numbers = |version: &str| -> Vec<u64> {
        version.split('.').map(|n| n.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0)).collect()
    };
    numbers(a) > numbers(b)
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compares_versions_by_number() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.4"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
    }
}
