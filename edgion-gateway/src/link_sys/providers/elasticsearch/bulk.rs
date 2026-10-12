//! Bulk ingest for Elasticsearch.
//!
//! Provides the background loop that receives documents via an mpsc channel,
//! batches them, and sends to ES via the `_bulk` API.
//! Handles retries with exponential backoff.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use edgion_common::link_sys_core::PublishedHealth;
use reqwest::Client;
use tokio::sync::{mpsc, watch};

use super::config_mapping::EsBulkConfig;
use crate::link_sys::runtime::health_probe::apply_health_result;

fn apply_published_health(
    healthy: &PublishedHealth,
    last_flip_log: &AtomicU64,
    name: &str,
    ok: bool,
    retired: &AtomicBool,
) {
    if retired.load(Ordering::Relaxed) {
        return;
    }
    apply_health_result(healthy, name, "elasticsearch", last_flip_log, ok, false);
}

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

#[derive(Debug, PartialEq, Eq)]
struct BulkItemSummary {
    retryable_indices: Vec<usize>,
    permanent_failures: usize,
}

/// Classify a parsed Bulk API response against the submitted documents.
///
/// Elasticsearch and OpenSearch return `items` in request order. Require an
/// exact one-to-one response before using that ordering so a malformed or
/// filtered response can never make us retry an already-indexed document.
/// Only 429 is retryable: it is the protocol's explicit overload signal.
fn classify_bulk_items(response: &BulkResponse, expected_items: usize) -> Result<BulkItemSummary, &'static str> {
    if response.items.len() != expected_items {
        return Err("response item count does not match request item count");
    }

    let mut retryable_indices = Vec::new();
    let mut permanent_failures = 0;
    for (index, item) in response.items.iter().enumerate() {
        let result = item.index.as_ref().ok_or("response item is not an index result")?;
        if result.status == 429 {
            retryable_indices.push(index);
        } else if result.status >= 400 {
            permanent_failures += 1;
        }
    }

    Ok(BulkItemSummary {
        retryable_indices,
        permanent_failures,
    })
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
    healthy: Arc<PublishedHealth>,
    last_flip_log: Arc<AtomicU64>,
    name: String,
    mut rx: mpsc::Receiver<String>,
    mut shutdown: watch::Receiver<bool>,
    retired: Arc<AtomicBool>,
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
                                &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown, &retired,
                            ).await;
                        }
                    }
                    None => {
                        // Channel closed — flush remaining and exit
                        if !buffer.is_empty() {
                            let batch = std::mem::take(&mut buffer);
                            let endpoint = &endpoints[endpoint_idx % endpoints.len().max(1)];
                            flush_batch(
                                &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown, &retired,
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
                        &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown, &retired,
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
                            &client, endpoint, &config, &healthy, &last_flip_log, &name, batch, &shutdown, &retired,
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
    healthy: &Arc<PublishedHealth>,
    last_flip_log: &AtomicU64,
    name: &str,
    docs: Vec<String>,
    shutdown: &watch::Receiver<bool>,
    retired: &AtomicBool,
) {
    let Ok(index_name) = config.current_index_name() else {
        tracing::error!(
            component = "link_sys",
            key_name = %name,
            dropped_documents = docs.len(),
            "Elasticsearch index date formatting failed; dropping batch"
        );
        return;
    };
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
        send_bulk_request(
            client,
            &bulk_url,
            sub_batch,
            &index_name,
            config,
            healthy,
            last_flip_log,
            name,
            shutdown,
            retired,
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

/// Wait for the retry delay while allowing shutdown to cancel it.
///
/// Returns `true` when the delay elapsed and another request may start. A
/// closed channel is treated like shutdown. Non-shutdown watch updates are
/// ignored without restarting the delay.
async fn wait_for_retry(backoff: std::time::Duration, shutdown: &watch::Receiver<bool>) -> bool {
    if *shutdown.borrow() {
        return false;
    }

    let sleep = tokio::time::sleep(backoff);
    tokio::pin!(sleep);
    let mut shutdown_wait = shutdown.clone();
    loop {
        tokio::select! {
            _ = &mut sleep => return true,
            changed = shutdown_wait.changed() => {
                if changed.is_err() || *shutdown_wait.borrow() {
                    return false;
                }
            }
        }
    }
}

/// Send one NDJSON body to ES via the Bulk API.
///
/// Handles retries with exponential backoff.
/// On permanent failure, logs error and drops the sub-batch.
// Passes each concern as a separate argument; a context struct would not
// improve clarity for a single private helper.
#[allow(clippy::too_many_arguments)]
async fn send_bulk_request(
    client: &Client,
    bulk_url: &str,
    docs: &[&str],
    index_name: &str,
    config: &EsBulkConfig,
    healthy: &Arc<PublishedHealth>,
    last_flip_log: &AtomicU64,
    name: &str,
    shutdown: &watch::Receiver<bool>,
    retired: &AtomicBool,
) {
    let mut pending_docs = docs.to_vec();
    let mut retries = 0u32;
    let mut backoff = config.backoff;

    loop {
        let body = build_ndjson_body(&pending_docs, index_name);
        match client
            .post(bulk_url)
            .header("Content-Type", "application/x-ndjson")
            .body(body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    match resp.json::<BulkResponse>().await {
                        Ok(bulk_resp) => {
                            if bulk_resp.errors {
                                let summary = match classify_bulk_items(&bulk_resp, pending_docs.len()) {
                                    Ok(summary) => summary,
                                    Err(reason) => {
                                        tracing::warn!("ES [{}] bulk response correlation error: {}", name, reason);
                                        // The server-side outcome is ambiguous. Do not retry the
                                        // whole request and risk duplicating successful documents.
                                        apply_published_health(healthy, last_flip_log, name, true, retired);
                                        return;
                                    }
                                };
                                let retryable_count = summary.retryable_indices.len();
                                let failed_count = retryable_count + summary.permanent_failures;
                                tracing::warn!(
                                    "ES [{}] bulk: {}/{} items failed ({} retryable)",
                                    name,
                                    failed_count,
                                    pending_docs.len(),
                                    retryable_count
                                );
                                if retryable_count == 0 {
                                    apply_published_health(healthy, last_flip_log, name, true, retired);
                                    return;
                                }
                                pending_docs = summary
                                    .retryable_indices
                                    .into_iter()
                                    .map(|index| pending_docs[index])
                                    .collect();
                            } else {
                                tracing::debug!(
                                    "ES [{}] bulk: {} items indexed successfully",
                                    name,
                                    pending_docs.len()
                                );
                                apply_published_health(healthy, last_flip_log, name, true, retired);
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("ES [{}] bulk response parse error: {:?}", name, e);
                            // Treat as success if HTTP status was 200 — items likely indexed
                            apply_published_health(healthy, last_flip_log, name, true, retired);
                            return;
                        }
                    }
                } else if status.is_server_error() {
                    tracing::warn!("ES [{}] bulk: server error {}", name, status);
                    apply_published_health(healthy, last_flip_log, name, false, retired);
                    // Fall through to retry
                } else {
                    // 4xx: client error, don't retry
                    tracing::error!("ES [{}] bulk: client error {}", name, status);
                    return;
                }
            }
            Err(e) => {
                tracing::warn!("ES [{}] bulk: network error: {:?}", name, e);
                apply_published_health(healthy, last_flip_log, name, false, retired);
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
                pending_docs.len()
            );
            apply_published_health(healthy, last_flip_log, name, false, retired);
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
                pending_docs.len()
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
        if !wait_for_retry(backoff, shutdown).await {
            tracing::info!(
                "ES [{}] bulk: shutdown signaled during retry backoff ({} items dropped)",
                name,
                pending_docs.len()
            );
            return;
        }
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
#[path = "../../../../tests/unit/link_sys/providers/elasticsearch/bulk_test.rs"]
mod tests;
