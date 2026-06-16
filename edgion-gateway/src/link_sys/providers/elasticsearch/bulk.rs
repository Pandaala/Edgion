//! Bulk ingest for Elasticsearch.
//!
//! Provides the background loop that receives documents via an mpsc channel,
//! batches them, and sends to ES via the `_bulk` API.
//! Handles retries with exponential backoff.

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::{mpsc, watch};

use super::config_mapping::EsBulkConfig;
use crate::link_sys::runtime::health_probe::apply_health_result;

// ============================================================================
// Bulk API response types
// ============================================================================

/// Elasticsearch Bulk API response
#[derive(serde::Deserialize, Debug)]
pub struct BulkResponse {
    pub took: u64,
    pub errors: bool,
    #[serde(default)]
    pub items: Vec<BulkResponseItem>,
}

#[derive(serde::Deserialize, Debug)]
pub struct BulkResponseItem {
    #[serde(default)]
    pub index: Option<BulkItemResult>,
}

#[derive(serde::Deserialize, Debug)]
pub struct BulkItemResult {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: Option<String>,
    pub status: u16,
    pub error: Option<serde_json::Value>,
}

// ============================================================================
// Bulk ingest loop
// ============================================================================

/// Background bulk ingest loop.
///
/// Receives docs via channel, batches them, and sends to ES via Bulk API.
/// Flushes on `batch_size` threshold or `flush_interval`, whichever comes first.
#[allow(clippy::too_many_arguments)]
pub async fn bulk_ingest_loop(
    client: Client,
    endpoints: Vec<String>,
    config: EsBulkConfig,
    healthy: Arc<AtomicBool>,
    last_flip_log: Arc<AtomicU64>,
    name: String,
    mut rx: mpsc::Receiver<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut buffer: Vec<String> = Vec::with_capacity(config.batch_size);
    let mut endpoint_idx: usize = 0;
    let flush_interval = tokio::time::interval(config.flush_interval);
    tokio::pin!(flush_interval);

    loop {
        tokio::select! {
            // Receive a new document
            doc = rx.recv() => {
                match doc {
                    Some(doc) => {
                        buffer.push(doc);
                        if buffer.len() >= config.batch_size {
                            let batch = std::mem::replace(
                                &mut buffer,
                                Vec::with_capacity(config.batch_size),
                            );
                            let endpoint = &endpoints[endpoint_idx % endpoints.len().max(1)];
                            endpoint_idx = endpoint_idx.wrapping_add(1);
                            flush_batch(
                                &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown,
                            ).await;
                        }
                    }
                    None => {
                        // Channel closed — flush remaining and exit
                        if !buffer.is_empty() {
                            let batch = std::mem::take(&mut buffer);
                            let endpoint = &endpoints[endpoint_idx % endpoints.len().max(1)];
                            flush_batch(
                                &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown,
                            ).await;
                        }
                        tracing::info!("ES [{}] bulk ingest loop exiting (channel closed)", name);
                        return;
                    }
                }
            }
            // Flush interval tick
            _ = flush_interval.tick() => {
                if !buffer.is_empty() {
                    let batch = std::mem::replace(
                        &mut buffer,
                        Vec::with_capacity(config.batch_size),
                    );
                    let endpoint = &endpoints[endpoint_idx % endpoints.len().max(1)];
                    endpoint_idx = endpoint_idx.wrapping_add(1);
                    flush_batch(
                        &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown,
                    ).await;
                }
            }
            // Shutdown signal
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    if !buffer.is_empty() {
                        let batch = std::mem::take(&mut buffer);
                        let endpoint = &endpoints[endpoint_idx % endpoints.len().max(1)];
                        flush_batch(
                            &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown,
                        ).await;
                    }
                    tracing::info!("ES [{}] bulk ingest loop exiting (shutdown)", name);
                    return;
                }
            }
        }
    }
}

/// Flush a batch of documents to ES via the Bulk API.
///
/// Partitions `docs` into sub-batches whose NDJSON body stays within
/// `config.max_body_bytes`, then sends each sub-batch independently. Without
/// this split the operator-configured ceiling is never enforced — a large
/// batch could exceed ES's `http.max_content_length` and get reset, feeding the
/// retry loop until the whole batch is lost. A single document that on its own
/// exceeds the limit can never be batched safely and is dropped with a warning.
/// In the common case (batch already within the limit) this produces exactly
/// one sub-batch, byte-identical to the unpartitioned path.
#[allow(clippy::too_many_arguments)]
async fn flush_batch(
    client: &Client,
    endpoint: &str,
    config: &EsBulkConfig,
    healthy: &Arc<AtomicBool>,
    last_flip_log: &AtomicU64,
    name: &str,
    docs: Vec<String>,
    shutdown: &watch::Receiver<bool>,
) {
    let index_name = config.current_index_name();
    let bulk_url = format!("{}/_bulk", endpoint);

    // Per-document wire cost: header line + '\n' + doc + '\n'. The header line
    // is constant for the batch, so its length is computed once here.
    let per_doc_overhead = ndjson_header_line(&index_name).len() + 2;

    let (sub_batches, dropped) = partition_into_sub_batches(&docs, per_doc_overhead, config.max_body_bytes);
    if dropped > 0 {
        tracing::warn!(
            "ES [{}] bulk: dropped {} document(s) each exceeding max_body_bytes ({})",
            name,
            dropped,
            config.max_body_bytes
        );
    }

    for sub_batch in &sub_batches {
        // The final drain (shutdown arm / channel-closed arm) calls this with
        // `shutdown` already signaled, so every sub-batch must still be sent to
        // honor the "flush buffered docs before exit" contract. Bounding the
        // post-shutdown work is left to `send_bulk_request` (which aborts its
        // retry loop on shutdown) plus the 10s drain timeout + task abort in
        // `EsLinkClient::shutdown`.
        let body = build_ndjson_body(sub_batch, &index_name);
        send_bulk_request(
            client,
            &bulk_url,
            body,
            config,
            healthy,
            last_flip_log,
            name,
            sub_batch.len(),
            shutdown,
        )
        .await;
    }
}

/// Partition `docs` into sub-batches whose NDJSON body stays within
/// `max_body_bytes`, using `per_doc_overhead` (header line + two newlines) plus
/// the document length as each document's wire cost.
///
/// Documents that individually exceed the limit cannot be batched safely and
/// are excluded; the second return value reports how many were dropped so the
/// caller can emit a single aggregated warning.
fn partition_into_sub_batches(
    docs: &[String],
    per_doc_overhead: usize,
    max_body_bytes: usize,
) -> (Vec<Vec<&str>>, usize) {
    let mut sub_batches: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_bytes = 0usize;
    let mut dropped = 0usize;

    for doc in docs {
        let doc_cost = per_doc_overhead + doc.len();
        if doc_cost > max_body_bytes {
            dropped += 1;
            continue;
        }
        if !current.is_empty() && current_bytes + doc_cost > max_body_bytes {
            sub_batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.push(doc);
        current_bytes += doc_cost;
    }
    if !current.is_empty() {
        sub_batches.push(current);
    }

    (sub_batches, dropped)
}

/// Send one NDJSON body to ES via the Bulk API.
///
/// Handles retries with exponential backoff.
/// On permanent failure, logs error and drops the sub-batch.
// Threads the request, its config, the health flag, the name, and the shutdown
// signal as separate borrows; bundling them into a context struct would not
// improve clarity for a single private helper.
#[allow(clippy::too_many_arguments)]
async fn send_bulk_request(
    client: &Client,
    bulk_url: &str,
    body: String,
    config: &EsBulkConfig,
    healthy: &Arc<AtomicBool>,
    last_flip_log: &AtomicU64,
    name: &str,
    doc_count: usize,
    shutdown: &watch::Receiver<bool>,
) {
    let mut retries = 0u32;
    let mut backoff = config.backoff;

    loop {
        match client
            .post(bulk_url)
            .header("Content-Type", "application/x-ndjson")
            .body(body.clone())
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    match resp.json::<BulkResponse>().await {
                        Ok(bulk_resp) => {
                            if bulk_resp.errors {
                                let failed_count = bulk_resp
                                    .items
                                    .iter()
                                    .filter(|item| item.index.as_ref().map(|i| i.status >= 400).unwrap_or(false))
                                    .count();
                                tracing::warn!("ES [{}] bulk: {}/{} items failed", name, failed_count, doc_count);
                            } else {
                                tracing::debug!("ES [{}] bulk: {} items indexed successfully", name, doc_count);
                            }
                            apply_health_result(healthy, name, "elasticsearch", last_flip_log, true, false);
                            return;
                        }
                        Err(e) => {
                            tracing::warn!("ES [{}] bulk response parse error: {:?}", name, e);
                            // Treat as success if HTTP status was 200 — items likely indexed
                            apply_health_result(healthy, name, "elasticsearch", last_flip_log, true, false);
                            return;
                        }
                    }
                } else if status.is_server_error() {
                    tracing::warn!("ES [{}] bulk: server error {}", name, status);
                    apply_health_result(healthy, name, "elasticsearch", last_flip_log, false, false);
                    // Fall through to retry
                } else {
                    // 4xx: client error, don't retry
                    tracing::error!("ES [{}] bulk: client error {}", name, status);
                    return;
                }
            }
            Err(e) => {
                tracing::warn!("ES [{}] bulk: network error: {:?}", name, e);
                apply_health_result(healthy, name, "elasticsearch", last_flip_log, false, false);
                // Fall through to retry
            }
        }

        // Retry with exponential backoff
        retries += 1;
        if retries > config.max_retries {
            tracing::error!(
                "ES [{}] bulk: max retries ({}) exhausted, {} items lost",
                name,
                config.max_retries,
                doc_count
            );
            return;
        }

        // Stop retrying promptly once shutdown is signaled. The retry loop is
        // otherwise unbounded relative to the 10s drain budget enforced by
        // EsLinkClient::shutdown: a slow/unreachable ES endpoint would let this
        // task outlive shutdown by up to max_retries * request_timeout.
        if *shutdown.borrow() {
            tracing::info!(
                "ES [{}] bulk: shutdown signaled, aborting retry ({} items dropped)",
                name,
                doc_count
            );
            return;
        }

        tracing::info!(
            "ES [{}] bulk: retry {}/{} after {:?}",
            name,
            retries,
            config.max_retries,
            backoff
        );
        tokio::time::sleep(backoff).await;
        backoff = backoff.mul_f64(2.0).min(std::time::Duration::from_secs(30));
    }
}

/// Build NDJSON body for ES Bulk API.
///
/// Format:
/// ```text
/// {"index":{"_index":"edgion-logs-2026.02.11"}}
/// {"message":"log line 1"}
/// {"index":{"_index":"edgion-logs-2026.02.11"}}
/// {"message":"log line 2"}
/// ```
pub fn build_ndjson_body<S: AsRef<str>>(docs: &[S], index_name: &str) -> String {
    let mut body = String::with_capacity(docs.len() * 256);
    let header_line = ndjson_header_line(index_name);
    for doc in docs {
        body.push_str(&header_line);
        body.push('\n');
        body.push_str(doc.as_ref());
        body.push('\n');
    }
    body
}

/// Build the per-document index-action header line for the ES Bulk API.
///
/// Built via serde_json so any special characters in `index_name` (e.g. `"` or
/// `\`) are correctly escaped and the NDJSON stays valid. The line is constant
/// for a given index name, so callers hoist it out of the per-document loop.
fn ndjson_header_line(index_name: &str) -> String {
    let header = serde_json::json!({ "index": { "_index": index_name } });
    serde_json::to_string(&header).unwrap_or_default()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ndjson_body_format() {
        let docs = vec![
            r#"{"message":"hello"}"#.to_string(),
            r#"{"message":"world"}"#.to_string(),
        ];
        let body = build_ndjson_body(&docs, "test-logs-2026.02.11");
        let lines: Vec<&str> = body.trim().split('\n').collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("\"index\""));
        assert!(lines[0].contains("test-logs-2026.02.11"));
        assert!(lines[1].contains("\"hello\""));
        assert!(lines[2].contains("\"index\""));
        assert!(lines[3].contains("\"world\""));
    }

    #[test]
    fn test_ndjson_body_escapes_special_chars_in_index_name() {
        // An index name containing JSON metacharacters must be escaped so the
        // header line stays valid NDJSON instead of breaking the wire protocol.
        let docs = vec![r#"{"message":"hello"}"#.to_string()];
        let body = build_ndjson_body(&docs, r#"weird"name\with-meta"#);
        let lines: Vec<&str> = body.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);
        // The header line must round-trip through serde_json with the exact name.
        let header: serde_json::Value = serde_json::from_str(lines[0]).expect("header is valid JSON");
        assert_eq!(header["index"]["_index"], r#"weird"name\with-meta"#);
        assert_eq!(lines[1], r#"{"message":"hello"}"#);
    }

    #[test]
    fn test_bulk_response_parse_success() {
        let json = r#"{"took":30,"errors":false,"items":[{"index":{"_index":"test","_id":"1","status":201}}]}"#;
        let resp: BulkResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.errors);
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].index.as_ref().unwrap().status, 201);
    }

    #[test]
    fn test_bulk_response_parse_with_errors() {
        let json = r#"{"took":30,"errors":true,"items":[{"index":{"_index":"test","_id":"1","status":201}},{"index":{"_index":"test","_id":"2","status":429,"error":{"type":"es_rejected_execution_exception"}}}]}"#;
        let resp: BulkResponse = serde_json::from_str(json).unwrap();
        assert!(resp.errors);
        let failed = resp
            .items
            .iter()
            .filter(|item| item.index.as_ref().map(|i| i.status >= 400).unwrap_or(false))
            .count();
        assert_eq!(failed, 1);
    }

    #[test]
    fn test_bulk_response_empty_items() {
        let json = r#"{"took":0,"errors":false,"items":[]}"#;
        let resp: BulkResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.errors);
        assert!(resp.items.is_empty());
    }

    #[test]
    fn test_partition_single_sub_batch_when_within_limit() {
        let docs = vec!["aaaa".to_string(), "bbbb".to_string(), "cccc".to_string()];
        // overhead 0 + 4 bytes each = 12 bytes total, well within the limit.
        let (batches, dropped) = partition_into_sub_batches(&docs, 0, 1024);
        assert_eq!(dropped, 0);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0], vec!["aaaa", "bbbb", "cccc"]);
    }

    #[test]
    fn test_partition_splits_on_byte_budget() {
        // Each doc costs per_doc_overhead(1) + len(4) = 5 bytes. With a 12-byte
        // limit, two docs (10 B) fit but a third (15 B) would exceed, so the
        // batch splits 2 + 1.
        let docs = vec![
            "aaaa".to_string(),
            "bbbb".to_string(),
            "cccc".to_string(),
            "dddd".to_string(),
        ];
        let (batches, dropped) = partition_into_sub_batches(&docs, 1, 12);
        assert_eq!(dropped, 0);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec!["aaaa", "bbbb"]);
        assert_eq!(batches[1], vec!["cccc", "dddd"]);
    }

    #[test]
    fn test_partition_drops_oversized_single_doc() {
        // The middle doc alone (overhead 1 + len 20 = 21 B) exceeds the 10-byte
        // limit and is dropped; the two small docs each fit in their own batch.
        let docs = vec!["aa".to_string(), "x".repeat(20), "bb".to_string()];
        let (batches, dropped) = partition_into_sub_batches(&docs, 1, 10);
        assert_eq!(dropped, 1);
        let kept: Vec<&str> = batches.iter().flatten().copied().collect();
        assert_eq!(kept, vec!["aa", "bb"]);
    }

    #[test]
    fn test_partition_empty_input() {
        let docs: Vec<String> = Vec::new();
        let (batches, dropped) = partition_into_sub_batches(&docs, 1, 10);
        assert_eq!(dropped, 0);
        assert!(batches.is_empty());
    }

    #[test]
    fn test_partition_doc_exactly_at_limit_is_kept() {
        // overhead 2 + len 8 = 10 bytes, exactly the limit — must be kept.
        let docs = vec!["x".repeat(8)];
        let (batches, dropped) = partition_into_sub_batches(&docs, 2, 10);
        assert_eq!(dropped, 0);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    /// link-sys-F016 regression: once the shutdown signal is set, the retry loop
    /// must exit before the next backoff sleep instead of exhausting `max_retries`.
    /// With a 5s backoff against an unreachable endpoint, the pre-fix code would
    /// sleep for seconds; the fix returns promptly after the first failed request,
    /// so the 2s guard below would trip if any retry sleep ran.
    #[tokio::test]
    async fn test_send_bulk_request_aborts_retry_on_shutdown() {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_millis(200))
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        let config = EsBulkConfig {
            batch_size: 500,
            flush_interval: std::time::Duration::from_secs(5),
            max_retries: 10,
            backoff: std::time::Duration::from_secs(5),
            max_body_bytes: 10 * 1024 * 1024,
            channel_size: 10_000,
            index_prefix: "test-logs".to_string(),
            date_pattern: "%Y.%m.%d".to_string(),
        };
        let healthy = Arc::new(AtomicBool::new(true));
        // Shutdown is already signaled before the call.
        let (_tx, shutdown_rx) = watch::channel(true);
        let body = build_ndjson_body(&[r#"{"message":"x"}"#], "test-logs-2026.01.01");

        // Port 1 is unreachable: the first request fails fast, after which the
        // shutdown check must return before the 5s backoff sleep.
        let last_flip = std::sync::atomic::AtomicU64::new(0);
        let fut = send_bulk_request(
            &client,
            "http://127.0.0.1:1/_bulk",
            body,
            &config,
            &healthy,
            &last_flip,
            "test",
            1,
            &shutdown_rx,
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("send_bulk_request must return promptly when shutdown is signaled");
    }
}
