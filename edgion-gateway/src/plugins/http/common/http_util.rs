//! Shared HTTP utilities for bounded JSON fetches.
//!
//! `fetch_json_limited` is the DoS-safe replacement for `resp.json::<T>().await`.
//! It enforces a byte-level upper bound on the response body in two tiers:
//!
//! 1. `Content-Length` pre-check — well-behaved servers are rejected before a
//!    single byte of body is read.
//! 2. Streaming accumulation via `bytes_stream()` — a chunked / dishonest server
//!    is cut off the moment the running total would exceed the limit, so the
//!    peak memory cost is at most `max_bytes + one_chunk`.
//!
//! Use this helper whenever an OIDC / OAuth / JWKS endpoint returns JSON that
//! is parsed into a trusted in-memory structure.

use futures::StreamExt;
use serde::de::DeserializeOwned;

/// Fetch and deserialize a JSON response body, refusing bodies larger than
/// `max_bytes`. `ctx` is a short label used in error messages (e.g. `"JWKS"`,
/// `"OIDC discovery"`) so failure logs identify the source.
pub async fn fetch_json_limited<T: DeserializeOwned>(
    resp: reqwest::Response,
    max_bytes: usize,
    ctx: &'static str,
) -> Result<T, String> {
    if let Some(cl) = resp.content_length() {
        if cl > max_bytes as u64 {
            return Err(format!(
                "{} response too large: Content-Length {} > limit {}",
                ctx, cl, max_bytes
            ));
        }
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(8192.min(max_bytes));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("{} body read error: {}", ctx, e))?;
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("{} response body exceeds limit {} bytes", ctx, max_bytes));
        }
        buf.extend_from_slice(&chunk);
    }

    serde_json::from_slice(&buf).map_err(|e| format!("Failed to parse {}: {}", ctx, e))
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/http_util_test.rs"]
mod tests;
