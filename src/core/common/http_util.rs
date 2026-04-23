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
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Doc {
        name: String,
    }

    async fn spawn_server_with_body(body: Vec<u8>, send_content_length: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut throwaway = [0u8; 1024];
            let _ = sock.read(&mut throwaway).await;
            let mut header = String::from("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n");
            if send_content_length {
                header.push_str(&format!("Content-Length: {}\r\n", body.len()));
            } else {
                header.push_str(&format!("Transfer-Encoding: chunked\r\n\r\n{:x}\r\n", body.len()));
            }
            if send_content_length {
                header.push_str("\r\n");
            }
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            if !send_content_length {
                let _ = sock.write_all(b"\r\n0\r\n\r\n").await;
            }
            let _ = sock.shutdown().await;
        });
        format!("http://{}/", addr)
    }

    #[tokio::test]
    async fn ok_within_limit() {
        let url = spawn_server_with_body(br#"{"name":"alice"}"#.to_vec(), true).await;
        let resp = reqwest::get(&url).await.unwrap();
        let doc: Doc = fetch_json_limited(resp, 1024, "test").await.unwrap();
        assert_eq!(doc, Doc { name: "alice".into() });
    }

    #[tokio::test]
    async fn content_length_rejects_early() {
        // 4096 bytes of body advertised via Content-Length, but limit is 128.
        let body = vec![b'x'; 4096];
        let url = spawn_server_with_body(body, true).await;
        let resp = reqwest::get(&url).await.unwrap();
        let err = fetch_json_limited::<Doc>(resp, 128, "test").await.unwrap_err();
        assert!(err.contains("Content-Length"), "got: {}", err);
    }

    #[tokio::test]
    async fn stream_rejects_dishonest_chunked() {
        // No Content-Length (chunked) — must be caught by streaming accumulator.
        let body = vec![b'y'; 4096];
        let url = spawn_server_with_body(body, false).await;
        let resp = reqwest::get(&url).await.unwrap();
        let err = fetch_json_limited::<Doc>(resp, 128, "test").await.unwrap_err();
        assert!(err.contains("exceeds limit"), "got: {}", err);
    }

    #[tokio::test]
    async fn parse_error_surfaces_with_ctx() {
        let url = spawn_server_with_body(b"not json".to_vec(), true).await;
        let resp = reqwest::get(&url).await.unwrap();
        let err = fetch_json_limited::<Doc>(resp, 1024, "testctx").await.unwrap_err();
        assert!(err.contains("Failed to parse testctx"), "got: {}", err);
    }
}
