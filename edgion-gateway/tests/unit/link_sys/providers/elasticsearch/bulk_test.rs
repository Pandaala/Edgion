use super::*;

// Observe only this worker's provider diagnostics while its future is polled
// under a scoped subscriber. No socket or server is needed for failure tests.
#[derive(Clone, Default)]
struct BulkEvents(Arc<parking_lot::Mutex<Vec<BulkEvent>>>);

#[derive(Default, Debug)]
struct BulkEvent {
    message: String,
    key_name: String,
    component: String,
    dropped_documents: Option<u64>,
}

impl tracing::field::Visit for BulkEvent {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "dropped_documents" {
            self.dropped_documents = Some(value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "key_name" => self.key_name = format!("{value:?}"),
            "component" => self.component = format!("{value:?}"),
            _ => {}
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BulkEvents {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "edgion_gateway::link_sys::providers::elasticsearch::bulk" {
            let mut observed = BulkEvent::default();
            event.record(&mut observed);
            self.0.lock().push(observed);
        }
    }
}

fn assert_date_format_drop(event: &BulkEvent, count: u64) {
    assert_eq!(
        event.message,
        "Elasticsearch index date formatting failed; dropping batch"
    );
    assert_eq!(event.key_name, "test/es");
    assert_eq!(event.component, "\"link_sys\"");
    assert_eq!(event.dropped_documents, Some(count));
}

#[tokio::test]
async fn invalid_date_format_drops_batch_without_request_retry_or_health_change() {
    use tracing_subscriber::layer::SubscriberExt;
    for initial_health in [false, true] {
        for pattern in ["%", "%#z"] {
            let events = BulkEvents::default();
            let subscriber = tracing_subscriber::registry().with(events.clone());
            let _guard = tracing::subscriber::set_default(subscriber);
            let mut config = test_bulk_config(10, std::time::Duration::from_secs(30));
            config.date_pattern = pattern.into();
            let health = Arc::new(PublishedHealth::new(initial_health));
            let last_flip = AtomicU64::new(0);
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);
            // reqwest rejects this sentinel before DNS/dialing if a regression
            // reaches the send path. The exact event count rejects that path.
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                flush_batch(
                    &Client::new(),
                    "not a URL",
                    &config,
                    &health,
                    &last_flip,
                    "test/es",
                    vec!["private-document-one".into(), "private-document-two".into()],
                    &shutdown_rx,
                    &AtomicBool::new(false),
                ),
            )
            .await
            .unwrap();
            assert_eq!(health.load(), initial_health);
            assert_eq!(last_flip.load(Ordering::Relaxed), 0);
            let observed = events.0.lock();
            assert_eq!(observed.len(), 1, "no request or retry diagnostics: {observed:?}");
            assert_date_format_drop(&observed[0], 2);
        }
    }
}

#[tokio::test]
async fn invalid_date_format_worker_processes_later_batches_and_channel_close() {
    use tracing_subscriber::layer::SubscriberExt;
    for initial_health in [false, true] {
        let events = BulkEvents::default();
        let subscriber = tracing_subscriber::registry().with(events.clone());
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut config = test_bulk_config(10, std::time::Duration::from_secs(30));
        config.batch_size = 1;
        config.flush_interval = std::time::Duration::from_secs(3600);
        config.date_pattern = "%#z".into();
        let health = Arc::new(PublishedHealth::new(initial_health));
        let last_flip = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel(1);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let worker = bulk_ingest_loop(
            Client::new(),
            vec!["not a URL".into()],
            config,
            Arc::clone(&health),
            Arc::clone(&last_flip),
            "test/es".into(),
            rx,
            shutdown_rx,
            Arc::new(AtomicBool::new(false)),
        );
        tokio::pin!(worker);
        for _ in 0..2 {
            tx.try_send("private-document".into()).unwrap();
            assert!(futures::poll!(&mut worker).is_pending());
        }
        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut worker)
            .await
            .unwrap();
        let observed = events.0.lock();
        assert_eq!(
            observed.len(),
            3,
            "two drops and normal channel-close exit: {observed:?}"
        );
        assert_date_format_drop(&observed[0], 1);
        assert_date_format_drop(&observed[1], 1);
        assert!(observed[2].message.contains("channel closed"));
        assert_eq!(health.load(), initial_health);
        assert_eq!(last_flip.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn invalid_date_format_shutdown_drops_buffered_batch_and_exits() {
    use tracing_subscriber::layer::SubscriberExt;
    let events = BulkEvents::default();
    let subscriber = tracing_subscriber::registry().with(events.clone());
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut config = test_bulk_config(10, std::time::Duration::from_secs(30));
    config.flush_interval = std::time::Duration::from_secs(3600);
    config.date_pattern = "%".into();
    let health = Arc::new(PublishedHealth::new(true));
    let last_flip = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel(1);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker = bulk_ingest_loop(
        Client::new(),
        vec!["not a URL".into()],
        config,
        Arc::clone(&health),
        Arc::clone(&last_flip),
        "test/es".into(),
        rx,
        shutdown_rx,
        Arc::new(AtomicBool::new(false)),
    );
    tokio::pin!(worker);
    // First poll consumes the initial empty interval tick. The next poll
    // consumes the document below the batch threshold with no pending tick.
    assert!(futures::poll!(&mut worker).is_pending());
    tx.try_send("private-document".into()).unwrap();
    assert!(futures::poll!(&mut worker).is_pending());
    assert!(events.0.lock().is_empty());
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), &mut worker)
        .await
        .unwrap();
    let observed = events.0.lock();
    assert_eq!(observed.len(), 2, "buffer drop and normal shutdown exit: {observed:?}");
    assert_date_format_drop(&observed[0], 1);
    assert!(observed[1].message.contains("shutdown"));
    assert!(health.load());
    assert_eq!(last_flip.load(Ordering::Relaxed), 0);
}

fn test_bulk_config(max_retries: u32, backoff: std::time::Duration) -> EsBulkConfig {
    EsBulkConfig {
        batch_size: 500,
        flush_interval: std::time::Duration::from_secs(5),
        max_retries,
        backoff,
        max_body_bytes: 10 * 1024 * 1024,
        max_response_bytes: 1024 * 1024,
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

#[tokio::test]
async fn receipt_cap_checks_declared_streamed_and_exact_limits() {
    let unpolled = futures::stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, ()>>> {
        panic!("declared oversize must not poll")
    });
    assert_eq!(
        read_bulk_receipt_stream(Some(11), unpolled, 10).await.unwrap_err(),
        BulkReceiptError::TooLarge
    );
    let json = Bytes::from_static(b"{\"errors\":false}");
    let exact = read_bulk_receipt_stream(
        Some(json.len() as u64),
        futures::stream::iter([Ok::<_, ()>(json.clone())]),
        json.len(),
    )
    .await
    .unwrap();
    assert!(!exact.errors);
    for declared in [None, Some(1)] {
        let chunks = futures::stream::iter([Ok::<_, ()>(json.clone()), Ok(Bytes::from_static(b"x"))]);
        assert_eq!(
            read_bulk_receipt_stream(declared, chunks, json.len())
                .await
                .unwrap_err(),
            BulkReceiptError::TooLarge
        );
    }
    let failed = futures::stream::iter([Ok(Bytes::from_static(b"{\"errors\":")), Err(())]);
    assert_eq!(
        read_bulk_receipt_stream(None, failed, 100).await.unwrap_err(),
        BulkReceiptError::Read
    );
    for malformed in [
        b"{\"errors\":false".as_slice(),
        b"{\"errors\":false}x",
        b"{\"errors\":true,\"items\":[{\"index\":{}}]}",
    ] {
        assert_eq!(
            read_bulk_receipt_stream(
                None,
                futures::stream::iter([Ok::<_, ()>(Bytes::copy_from_slice(malformed))]),
                100
            )
            .await
            .unwrap_err(),
            BulkReceiptError::Parse
        );
    }
}

#[tokio::test]
async fn receipt_projection_ignores_large_metadata_without_retaining_error_tree() {
    let depth = 1024;
    let nested = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
    let json = format!(
        r#"{{"took":"ignored","errors":true,"items":[{{"index":{{"status":429,"_index":false,"_id":{{}},"error":{{"reason":"{}","caused_by":{}}}}}}}]}}"#,
        "x".repeat(256 * 1024),
        nested
    );
    let cap = json.len();
    let response = read_bulk_receipt_stream(None, futures::stream::iter([Ok::<_, ()>(Bytes::from(json))]), cap)
        .await
        .unwrap();
    assert_eq!(classify_bulk_items(&response, 1).unwrap().retryable_indices, vec![0]);
    assert_eq!(
        format!("{response:?}"),
        "BulkResponse { errors: true, items: [BulkResponseItem { index: Some(BulkItemResult { status: 429 }) }] }"
    );
    let broken = format!(r#"{{"errors":false,"unknown":{}0}}"#, "[".repeat(depth));
    assert_eq!(
        read_bulk_receipt_stream(None, futures::stream::iter([Ok::<_, ()>(Bytes::from(broken))]), cap)
            .await
            .unwrap_err(),
        BulkReceiptError::Parse
    );
}

#[test]
fn receipt_projection_keeps_required_relevant_fields_and_rejects_duplicates() {
    for malformed in [
        r#"{"items":[]}"#,
        r#"{"errors":"false"}"#,
        r#"{"errors":false,"errors":true}"#,
        r#"{"errors":true,"items":[{"index":{"status":65536}}]}"#,
        r#"{"errors":true,"items":[{"index":{"status":"429"}}]}"#,
        r#"{"errors":true,"items":[{"index":{"status":429,"status":201}}]}"#,
    ] {
        assert!(serde_json::from_str::<BulkResponse>(malformed).is_err(), "{malformed}");
    }
}

fn scripted_response(status: u16, body: impl Into<reqwest::Body>) -> reqwest::Response {
    http::Response::builder()
        .status(status)
        .body(body.into())
        .unwrap()
        .into()
}

#[tokio::test]
async fn scripted_bulk_ambiguous_receipts_do_not_replay_and_obey_retirement_fence() {
    for initial_health in [false, true] {
        for retired in [false, true] {
            for receipt in [
                r#"{"errors":false}trailing"#,
                r#"{"errors":true,"items":[]}"#,
                r#"{"errors":true,"items":[{"delete":{"status":429}}]}"#,
                "oversized-response-content",
            ] {
                let mut config = test_bulk_config(3, std::time::Duration::ZERO);
                config.max_response_bytes = if receipt == "oversized-response-content" {
                    8
                } else {
                    1000
                };
                let health = Arc::new(PublishedHealth::new(initial_health));
                let flip = AtomicU64::new(0);
                let (_tx, rx) = watch::channel(false);
                let mut attempts = 0;
                send_bulk_request_using(
                    |_| {
                        attempts += 1;
                        std::future::ready(Ok(scripted_response(200, receipt.to_owned())))
                    },
                    &["doc"],
                    "test",
                    &config,
                    &health,
                    &flip,
                    "test/es",
                    &rx,
                    &AtomicBool::new(retired),
                )
                .await;
                assert_eq!(attempts, 1);
                assert_eq!(health.load(), initial_health || !retired);
            }
        }
    }
}

#[tokio::test]
async fn scripted_bulk_read_failure_is_ambiguous_and_errors_false_bypasses_correlation() {
    let config = test_bulk_config(3, std::time::Duration::ZERO);
    let health = Arc::new(PublishedHealth::new(false));
    let flip = AtomicU64::new(0);
    let (_tx, rx) = watch::channel(false);
    let mut attempts = 0;
    send_bulk_request_using(
        |_| {
            attempts += 1;
            let failed = futures::stream::iter([Err::<Bytes, _>(std::io::Error::other("private-body-error"))]);
            std::future::ready(Ok(scripted_response(200, reqwest::Body::wrap_stream(failed))))
        },
        &["doc"],
        "test",
        &config,
        &health,
        &flip,
        "test/es",
        &rx,
        &AtomicBool::new(false),
    )
    .await;
    assert_eq!(attempts, 1);
    assert!(health.load());
    send_bulk_request_using(
        |_| {
            attempts += 1;
            std::future::ready(Ok(scripted_response(200, r#"{"errors":false,"items":[]}"#)))
        },
        &["doc"],
        "test",
        &config,
        &health,
        &flip,
        "test/es",
        &rx,
        &AtomicBool::new(false),
    )
    .await;
    assert_eq!(attempts, 2);
    assert!(health.load());
}

#[tokio::test]
async fn scripted_bulk_mixed_5xx_and_429_share_budget_and_keep_ordered_subset() {
    let config = test_bulk_config(2, std::time::Duration::ZERO);
    let health = Arc::new(PublishedHealth::new(true));
    let flip = AtomicU64::new(0);
    let (_tx, rx) = watch::channel(false);
    let mut responses = std::collections::VecDeque::from([
        scripted_response(503, "unavailable"),
        scripted_response(
            200,
            r#"{"errors":true,"items":[{"index":{"status":201}},{"index":{"status":429}},{"index":{"status":400}},{"index":{"status":429}}]}"#,
        ),
        scripted_response(
            200,
            r#"{"errors":true,"items":[{"index":{"status":429}},{"index":{"status":429}}]}"#,
        ),
    ]);
    let mut bodies = Vec::new();
    send_bulk_request_using(
        |body| {
            bodies.push(body);
            std::future::ready(Ok(responses.pop_front().expect("shared budget exceeded")))
        },
        &["success", "retry-first", "permanent", "retry-second"],
        "test",
        &config,
        &health,
        &flip,
        "test/es",
        &rx,
        &AtomicBool::new(false),
    )
    .await;
    assert!(responses.is_empty());
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(bodies[2], build_ndjson_body(&["retry-first", "retry-second"], "test"));
    assert!(!health.load());
}

struct PendingReceipt(Arc<AtomicBool>);
impl futures::Stream for PendingReceipt {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Pending
    }
}
impl Drop for PendingReceipt {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn cancelling_pending_bulk_reception_drops_stream_without_replay_or_health_change() {
    let config = test_bulk_config(3, std::time::Duration::ZERO);
    let health = Arc::new(PublishedHealth::new(false));
    let flip = AtomicU64::new(0);
    let (_tx, rx) = watch::channel(false);
    let dropped = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let retired = AtomicBool::new(false);
    let mut future = Box::pin(send_bulk_request_using(
        |_| {
            attempts.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(scripted_response(
                200,
                reqwest::Body::wrap_stream(PendingReceipt(Arc::clone(&dropped))),
            )))
        },
        &["doc"],
        "test",
        &config,
        &health,
        &flip,
        "test/es",
        &rx,
        &retired,
    ));
    assert!(futures::poll!(&mut future).is_pending());
    drop(future);
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(!health.load());
}
