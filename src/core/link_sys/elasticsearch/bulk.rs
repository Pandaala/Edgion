//! Bulk ingest for Elasticsearch.
//!
//! Provides the background loop that receives documents via an mpsc channel,
//! batches them, and sends to ES via the `_bulk` API.
//! Handles retries with exponential backoff.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::{mpsc, watch};

use super::config_mapping::EsBulkConfig;

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
pub async fn bulk_ingest_loop(
    client: Client,
    endpoints: Vec<String>,
    config: EsBulkConfig,
    healthy: Arc<AtomicBool>,
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
                                &client, endpoint, &config, &healthy, &name, batch,
                            ).await;
                        }
                    }
                    None => {
                        // Channel closed — flush remaining and exit
                        if !buffer.is_empty() {
                            let batch = std::mem::take(&mut buffer);
                            let endpoint = &endpoints[endpoint_idx % endpoints.len().max(1)];
                            flush_batch(
                                &client, endpoint, &config, &healthy, &name, batch,
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
                        &client, endpoint, &config, &healthy, &name, batch,
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
                            &client, endpoint, &config, &healthy, &name, batch,
                        ).await;
                    }
                    tracing::info!("ES [{}] bulk ingest loop exiting (shutdown)", name);
                    return;
                }
            }
        }
    }
}

/// Flush a batch of documents to ES via Bulk API.
///
/// Handles retries with exponential backoff.
/// On permanent failure, logs error (TODO: integrate FailedCache).
async fn flush_batch(
    client: &Client,
    endpoint: &str,
    config: &EsBulkConfig,
    healthy: &Arc<AtomicBool>,
    name: &str,
    docs: Vec<String>,
) {
    let index_name = config.current_index_name();
    let bulk_url = format!("{}/_bulk", endpoint);

    // Build NDJSON body
    let body = build_ndjson_body(&docs, &index_name);

    let mut retries = 0u32;
    let mut backoff = config.backoff;

    loop {
        match client
            .post(&bulk_url)
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
                                tracing::warn!("ES [{}] bulk: {}/{} items failed", name, failed_count, docs.len());
                                // TODO: Collect failed items and send to FailedCache
                            } else {
                                tracing::debug!("ES [{}] bulk: {} items indexed successfully", name, docs.len());
                            }
                            healthy.store(true, Ordering::Relaxed);
                            return;
                        }
                        Err(e) => {
                            tracing::warn!("ES [{}] bulk response parse error: {:?}", name, e);
                            // Treat as success if HTTP status was 200 — items likely indexed
                            healthy.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                } else if status.is_server_error() {
                    tracing::warn!("ES [{}] bulk: server error {}", name, status);
                    healthy.store(false, Ordering::Relaxed);
                    // Fall through to retry
                } else {
                    // 4xx: client error, don't retry
                    tracing::error!("ES [{}] bulk: client error {}", name, status);
                    // TODO: Send to FailedCache
                    return;
                }
            }
            Err(e) => {
                tracing::warn!("ES [{}] bulk: network error: {:?}", name, e);
                healthy.store(false, Ordering::Relaxed);
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
                docs.len()
            );
            // TODO: Send to FailedCache (LocalFileWriter or Redis)
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
pub fn build_ndjson_body(docs: &[String], index_name: &str) -> String {
    let mut body = String::with_capacity(docs.len() * 256);
    for doc in docs {
        body.push_str(&format!(r#"{{"index":{{"_index":"{}"}}}}"#, index_name));
        body.push('\n');
        body.push_str(doc);
        body.push('\n');
    }
    body
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
}
