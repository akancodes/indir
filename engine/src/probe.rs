//! The first question to the server: how big is the file, will it send the
//! file in parts, and what is it called?

use std::time::Duration;

use reqwest::header::{CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_TYPE, ETAG, LAST_MODIFIED, RANGE};
use reqwest::{Client, StatusCode};
use url::Url;

use crate::name;
use crate::transfer::{describe, http_failure, Stop};

/// How long the first answer may take. As a reqwest request timeout it also
/// covers the body, which is fine here: the probe reads none of it.
const ANSWER: Duration = Duration::from_secs(30);

pub(crate) struct Probe {
    /// Where the link led after redirects; the parts are fetched from here.
    pub url: Url,
    pub size: Option<u64>,
    /// Whether the server sends a byte range when asked for one.
    pub ranges: bool,
    pub name: String,
    /// The ETag or Last-Modified date, to notice the file changing between sessions.
    pub validator: Option<String>,
}

/// Asks for the first byte only. A `206 Partial Content` answer means ranges
/// work, and its `Content-Range` says how big the whole file is.
pub(crate) async fn probe(client: &Client, url: &Url) -> Result<Probe, Stop> {
    let response = client
        .get(url.clone())
        .header(RANGE, "bytes=0-0")
        .timeout(ANSWER)
        .send()
        .await
        .map_err(|e| Stop::Retry(describe(&e)))?;
    let status = response.status();
    let headers = response.headers();
    let total = headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(content_range_total);
    let (size, ranges) = match status {
        StatusCode::PARTIAL_CONTENT => (total, total.is_some()),
        // Only an empty file has no first byte to send.
        StatusCode::RANGE_NOT_SATISFIABLE if total == Some(0) => (Some(0), true),
        status if status.is_success() => (response.content_length(), false),
        status => return Err(http_failure(status)),
    };
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let name = name::suggested(headers.get(CONTENT_DISPOSITION).map(|v| v.as_bytes()), response.url(), content_type);
    let validator =
        headers.get(ETAG).or_else(|| headers.get(LAST_MODIFIED)).and_then(|v| v.to_str().ok()).map(str::to_string);
    Ok(Probe { url: response.url().clone(), size, ranges, name, validator })
}

/// "bytes 0-0/12345" → 12345, "bytes */0" → 0, "bytes 0-0/*" → unknown.
pub(crate) fn content_range_total(value: &str) -> Option<u64> {
    value.trim().strip_prefix("bytes")?.rsplit('/').next()?.trim().parse().ok()
}

/// "bytes 100-199/1000" → 100.
pub(crate) fn content_range_start(value: &str) -> Option<u64> {
    value.trim().strip_prefix("bytes")?.trim_start().split('-').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_content_range() {
        assert_eq!(content_range_total("bytes 0-0/12345"), Some(12345));
        assert_eq!(content_range_total("bytes */0"), Some(0));
        assert_eq!(content_range_total("bytes 0-0/*"), None);
        assert_eq!(content_range_total("items 0-0/5"), None);
        assert_eq!(content_range_start("bytes 100-199/1000"), Some(100));
        assert_eq!(content_range_start("bytes 0-0/*"), Some(0));
        assert_eq!(content_range_start("bytes */1000"), None);
    }
}
