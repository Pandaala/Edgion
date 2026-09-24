use super::*;

fn test_bulk_config(max_retries: u32, backoff: std::time::Duration) -> EsBulkConfig {
    EsBulkConfig {
        batch_size: 500,
        flush_interval: std::time::Duration::from_secs(5),
        max_retries,
        backoff,
        max_body_bytes: 10 * 1024 * 1024,
        channel_size: 10_000,
        index_prefix: "test-logs".to_string(),
        date_pattern: "%Y.%m.%d".to_string(),
    }
}

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
fn test_classify_bulk_items_selects_only_429_in_request_order() {
    let json = r#"{"took":30,"errors":true,"items":[{"index":{"_index":"test","status":201}},{"index":{"_index":"test","status":429,"error":{"type":"es_rejected_execution_exception"}}},{"index":{"_index":"test","status":400,"error":{"type":"mapper_parsing_exception"}}},{"index":{"_index":"test","status":429,"error":{"type":"rejected_execution_exception"}}}]}"#;
    let response: BulkResponse = serde_json::from_str(json).unwrap();

    let summary = classify_bulk_items(&response, 4).unwrap();

    assert_eq!(summary.retryable_indices, vec![1, 3]);
    assert_eq!(summary.permanent_failures, 1);
}

#[test]
fn test_classify_bulk_items_rejects_unsafe_correlation() {
    let wrong_count: BulkResponse =
        serde_json::from_str(r#"{"took":1,"errors":true,"items":[{"index":{"_index":"test","status":429}}]}"#).unwrap();
    assert_eq!(
        classify_bulk_items(&wrong_count, 2),
        Err("response item count does not match request item count")
    );

    let wrong_action: BulkResponse =
        serde_json::from_str(r#"{"took":1,"errors":true,"items":[{"delete":{}}]}"#).unwrap();
    assert_eq!(
        classify_bulk_items(&wrong_action, 1),
        Err("response item is not an index result")
    );
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

#[tokio::test]
async fn test_send_bulk_request_retries_only_429_items() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    Mock::given(method("POST"))
        .and(path("/_bulk"))
        .respond_with(move |_request: &wiremock::Request| {
            if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "took": 1,
                    "errors": true,
                    "items": [
                        {"index": {"_index": "test", "status": 201}},
                        {"index": {"_index": "test", "status": 429,
                            "error": {"type": "es_rejected_execution_exception"}}}
                    ]
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "took": 1,
                    "errors": false,
                    "items": [{"index": {"_index": "test", "status": 201}}]
                }))
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let docs = [r#"{"message":"accepted"}"#, r#"{"message":"retry-me"}"#];
    let healthy = Arc::new(PublishedHealth::new(true));
    let last_flip = AtomicU64::new(0);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    send_bulk_request(
        &Client::new(),
        &format!("{}/_bulk", server.uri()),
        &docs,
        "test",
        &test_bulk_config(3, std::time::Duration::ZERO),
        &healthy,
        &last_flip,
        "test",
        &shutdown_rx,
        &AtomicBool::new(false),
    )
    .await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let first_body = std::str::from_utf8(&requests[0].body).unwrap();
    let retry_body = std::str::from_utf8(&requests[1].body).unwrap();
    assert!(first_body.contains("accepted"));
    assert!(first_body.contains("retry-me"));
    assert!(!retry_body.contains("accepted"));
    assert!(retry_body.contains("retry-me"));
    assert!(healthy.load());
}

#[tokio::test]
async fn test_send_bulk_request_degrades_after_429_retry_budget_is_exhausted() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/_bulk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "took": 1,
            "errors": true,
            "items": [{"index": {"_index": "test", "status": 429,
                "error": {"type": "es_rejected_execution_exception"}}}]
        })))
        .expect(3)
        .mount(&server)
        .await;

    let docs = [r#"{"message":"retry-me"}"#];
    let healthy = Arc::new(PublishedHealth::new(true));
    let last_flip = AtomicU64::new(0);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    send_bulk_request(
        &Client::new(),
        &format!("{}/_bulk", server.uri()),
        &docs,
        "test",
        &test_bulk_config(2, std::time::Duration::ZERO),
        &healthy,
        &last_flip,
        "test",
        &shutdown_rx,
        &AtomicBool::new(false),
    )
    .await;

    assert!(!healthy.load());
}

#[tokio::test]
async fn test_retry_backoff_is_cancelled_after_wait_starts() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let wait = wait_for_retry(std::time::Duration::from_secs(30), &shutdown_rx);
    tokio::pin!(wait);

    // Poll the helper to Pending first, proving that the shutdown signal is
    // delivered after the backoff wait has started.
    tokio::select! {
        biased;
        result = &mut wait => panic!("backoff unexpectedly completed: {result}"),
        _ = tokio::task::yield_now() => {}
    }
    shutdown_tx.send(true).unwrap();
    let elapsed = tokio::time::timeout(std::time::Duration::from_secs(1), &mut wait)
        .await
        .expect("shutdown must cancel the in-progress retry backoff");

    assert!(!elapsed);
}

/// Once shutdown is signaled the retry loop must exit before the next backoff
/// sleep. With a 5s backoff and an unreachable endpoint, the 2s timeout below
/// trips if any retry sleep runs.
#[tokio::test]
async fn test_send_bulk_request_aborts_retry_on_shutdown() {
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_millis(200))
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    let config = test_bulk_config(10, std::time::Duration::from_secs(5));
    let healthy = Arc::new(PublishedHealth::new(true));
    // Shutdown is already signaled before the call.
    let (_tx, shutdown_rx) = watch::channel(true);
    let docs = [r#"{"message":"x"}"#];

    // Port 1 is unreachable: the first request fails fast, after which the
    // shutdown check must return before the 5s backoff sleep.
    let last_flip = std::sync::atomic::AtomicU64::new(0);
    let retired = AtomicBool::new(false);
    let fut = send_bulk_request(
        &client,
        "http://127.0.0.1:1/_bulk",
        &docs,
        "test-logs-2026.01.01",
        &config,
        &healthy,
        &last_flip,
        "test",
        &shutdown_rx,
        &retired,
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), fut)
        .await
        .expect("send_bulk_request must return promptly when shutdown is signaled");
}
