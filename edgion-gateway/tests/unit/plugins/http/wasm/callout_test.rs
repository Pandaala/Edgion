use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn callout(origin: String, timeout_ms: u32) -> PendingCallout {
    PendingCallout {
        token: 7,
        origin,
        path: "/callout".to_string(),
        method: "GET".to_string(),
        authority: None,
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms,
    }
}

fn test_client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().expect("test client")
}

#[test]
fn request_builder_rebuilds_body_framing_and_preserves_guest_headers() {
    let client = test_client();
    let callout = PendingCallout {
        token: 11,
        origin: "https://example.test:8443".to_string(),
        path: "/v1/check?tenant=blue".to_string(),
        method: "POST".to_string(),
        authority: Some("service.internal:9443".to_string()),
        headers: vec![
            ("content-length".to_string(), "999".to_string()),
            ("content-length".to_string(), "1".to_string()),
            ("transfer-encoding".to_string(), "chunked".to_string()),
            ("x-order".to_string(), "first".to_string()),
            ("bad header".to_string(), "skipped".to_string()),
            ("x-invalid".to_string(), "bad\nvalue".to_string()),
            ("x-order".to_string(), "second".to_string()),
        ],
        body: b"payload".to_vec(),
        timeout_ms: 500,
    };

    let (token, req) = build_callout_request(callout, &client).expect("callout request should build");

    assert_eq!(token, 11);
    assert_eq!(req.url().as_str(), "https://example.test:8443/v1/check?tenant=blue");
    assert_eq!(req.method(), reqwest::Method::POST);
    assert_eq!(
        req.body().and_then(reqwest::Body::as_bytes),
        Some(b"payload".as_slice())
    );
    assert_eq!(
        req.headers()
            .get(reqwest::header::HOST)
            .and_then(|value| value.to_str().ok()),
        Some("service.internal:9443")
    );
    assert_eq!(
        req.headers()
            .get_all(reqwest::header::CONTENT_LENGTH)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>(),
        vec!["7"]
    );
    assert!(req.headers().get(reqwest::header::TRANSFER_ENCODING).is_none());
    assert_eq!(
        req.headers()
            .get_all("x-order")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert!(req.headers().get("bad header").is_none());
    assert!(req.headers().get("x-invalid").is_none());
}

#[test]
fn bodyless_request_removes_guest_framing_without_adding_zero_length() {
    let client = test_client();
    let mut callout = callout("https://example.test".to_string(), 500);
    callout.headers = vec![
        ("content-length".to_string(), "99".to_string()),
        ("transfer-encoding".to_string(), "chunked".to_string()),
    ];

    let (_, req) = build_callout_request(callout, &client).expect("bodyless callout request should build");

    assert!(req.body().is_none());
    assert!(req.headers().get(reqwest::header::CONTENT_LENGTH).is_none());
    assert!(req.headers().get(reqwest::header::TRANSFER_ENCODING).is_none());
}

#[tokio::test]
async fn unavailable_guarded_client_fails_without_network() {
    let server = wiremock::MockServer::start().await;
    let callout = callout(server.uri(), 1_000);
    let token = callout.token;
    let permits = tokio::sync::Semaphore::new(1);

    let response = execute_callout_with_client_and_permits(callout, 1_000, "test", Err(()), &permits, None).await;

    assert_eq!(response.token, token);
    assert!(response.headers.is_empty());
    assert!(response.body.is_empty());
    assert!(
        server.received_requests().await.expect("read mock requests").is_empty(),
        "client initialization failure must not fall back to an unguarded request"
    );
}

#[tokio::test]
async fn zero_permit_semaphore_is_bounded_by_callout_deadline() {
    let client = test_client();
    let permits = tokio::sync::Semaphore::new(0);
    let callout = callout("http://127.0.0.1:9".to_string(), 0);

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        execute_callout_with_client_and_permits(callout, 25, "test", Ok(&client), &permits, None),
    )
    .await
    .expect("callout deadline must bound semaphore wait");

    assert!(response.headers.is_empty());
    assert!(response.body.is_empty());
}

#[tokio::test]
async fn held_permit_until_deadline_prevents_network_request() {
    let server = wiremock::MockServer::start().await;
    let client = test_client();
    let permits = tokio::sync::Semaphore::new(1);
    let held = permits.acquire().await.expect("hold only permit");
    let callout = callout(server.uri(), 30);

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        execute_callout_with_client_and_permits(callout, 1_000, "test", Ok(&client), &permits, None),
    )
    .await
    .expect("callout deadline must expire");

    assert!(response.headers.is_empty());
    assert!(response.body.is_empty());
    assert!(server.received_requests().await.expect("read mock requests").is_empty());
    drop(held);
}

#[tokio::test]
async fn permit_released_before_deadline_allows_real_callout() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
        .mount(&server)
        .await;
    let client = test_client();
    let permits = tokio::sync::Semaphore::new(1);
    let held = permits.acquire().await.expect("hold only permit");
    let callout = callout(server.uri(), 1_000);

    let run_callout = execute_callout_with_client_and_permits(callout, 1_000, "test", Ok(&client), &permits, None);
    let release = async move {
        tokio::task::yield_now().await;
        drop(held);
    };
    let (response, ()) = tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(run_callout, release) })
        .await
        .expect("released permit must unblock callout");

    assert_eq!(response.headers.first().map(|h| h.1.as_str()), Some("200"));
    assert_eq!(response.body, b"ok");
}

#[tokio::test]
async fn body_stream_timeout_drops_in_flight_permit_without_leak() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("test server address");
    let (headers_sent_tx, headers_sent_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept callout");
        let mut request = [0u8; 1024];
        let bytes_read = stream.read(&mut request).await.expect("read callout");
        assert!(bytes_read > 0, "callout request must not be empty");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
            .await
            .expect("write response headers");
        let _ = headers_sent_tx.send(());
        std::future::pending::<()>().await;
    });

    let client = std::sync::Arc::new(test_client());
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let callout = callout(format!("http://{addr}"), 500);
    let call_client = client.clone();
    let call_permits = permits.clone();
    let call_task = tokio::spawn(async move {
        execute_callout_with_client_and_permits(callout, 1_000, "test", Ok(&call_client), &call_permits, None).await
    });

    tokio::time::timeout(Duration::from_secs(2), headers_sent_rx)
        .await
        .expect("callout must receive response headers before its deadline")
        .expect("test server must report sent headers");
    let response = tokio::time::timeout(Duration::from_secs(2), call_task)
        .await
        .expect("body stream must obey callout deadline")
        .expect("callout task must not panic");

    assert!(response.headers.is_empty());
    assert!(response.body.is_empty());
    let reacquired = tokio::time::timeout(Duration::from_secs(1), permits.acquire())
        .await
        .expect("timed-out callout must release permit")
        .expect("semaphore remains open");
    drop(reacquired);
    server_task.abort();
}

#[tokio::test]
async fn early_resume_counts_taken_remainder_and_poisoned_vm_is_not_reused() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::callout::tests",
        "early_resume_counts_taken_remainder_and_poisoned_vm_is_not_reused",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "early_resume_counts_taken_remainder_and_poisoned_vm_is_not_reused",
        || async {
            use super::super::host_abi::WasmPhase;
            use super::super::vm_manager::{tests::actual_runtime, CalloutGrant, WasmVmManager};
            use edgion_resources::plugin_configs::WasmConfig;
            let guest = r#"(module
                (import "env" "proxy_continue_request" (func $resume (result i32)))
                (memory (export "memory") 1)
                (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) i32.const 1)
                (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32) call $resume drop))"#;
            let file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(file.path(), wat::parse_str(guest).unwrap()).unwrap();
            let mut config = WasmConfig::default();
            config.source.file = Some(file.path().to_string_lossy().into_owned());
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
            let entry = runtime.get_or_load().await.unwrap();
            let mut lease = entry.acquire(100).unwrap();
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], Default::default());
            lease.open_request(0, true).unwrap();
            for token in 1..=3 {
                let mut pending = callout("invalid URL".into(), 100);
                pending.token = token;
                lease.state_mut().frame.pending_callouts.push(pending);
            }
            let id = lease.context_id().unwrap();
            let outcome = resolve_callouts_on(&mut lease, id, 100, "callout-test", None)
                .await
                .unwrap();
            assert_eq!((outcome.delivered, outcome.leftover), (1, 2));
            assert!(!lease.is_healthy());
            drop(lease);
            assert_eq!(entry.pool_len(), 0);
        },
    )
    .await;
}

#[tokio::test]
async fn expired_request_and_waiting_cancellation_do_not_consume_network_capacity() {
    let permits = tokio::sync::Semaphore::new(0);
    let expired = tokio::time::Instant::now();
    let response = execute_callout_with_client_and_permits(
        callout("https://example.test".into(), 1000),
        1000,
        "deadline-test",
        Err(()),
        &permits,
        Some(expired),
    )
    .await;
    assert!(response.headers.is_empty());
    assert_eq!(response.token, 7);
    let future = execute_callout_with_client_and_permits(
        callout("https://example.test".into(), 1000),
        1000,
        "deadline-test",
        Err(()),
        &permits,
        None,
    );
    let mut future = Box::pin(future);
    assert!(futures::poll!(future.as_mut()).is_pending());
    drop(future);
    permits.add_permits(1);
    let held = permits.try_acquire().expect("cancelled waiter must be removed");
    // Timeout also cancels a waiter without creating a deliverable late callback.
    let response = execute_callout_with_client_and_permits(
        callout("https://example.test".into(), 1000),
        1000,
        "deadline-test",
        Err(()),
        &permits,
        Some(tokio::time::Instant::now() + Duration::from_millis(1)),
    )
    .await;
    assert!(response.headers.is_empty());
    drop(held);
}

#[test]
fn response_header_snapshot_rejects_bytes_and_count_before_copying() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x",
        reqwest::header::HeaderValue::from_bytes(&vec![b'a'; 65535]).unwrap(),
    );
    assert!(validate_response_headers(&headers).is_ok());
    headers.append("x", reqwest::header::HeaderValue::from_static("a"));
    assert!(validate_response_headers(&headers).is_err());
    headers.clear();
    for _ in 0..1024 {
        headers.append("x", reqwest::header::HeaderValue::from_static(""));
    }
    assert!(validate_response_headers(&headers).is_ok());
    headers.append("x", reqwest::header::HeaderValue::from_static(""));
    assert!(validate_response_headers(&headers).is_err());
}

#[tokio::test]
async fn expired_request_abandons_tokens_without_guest_callback() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::callout::tests",
        "expired_request_abandons_tokens_without_guest_callback",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "expired_request_abandons_tokens_without_guest_callback",
        || async {
            use super::super::vm_manager::{tests::actual_runtime, CalloutGrant, WasmVmManager};
            let guest = r#"(module (memory (export "memory") 1)
                (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) i32.const 1)
                (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32) unreachable))"#;
            let file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(file.path(), wat::parse_str(guest).unwrap()).unwrap();
            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            config.source.file = Some(file.path().to_string_lossy().into_owned());
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
            let entry = runtime.get_or_load().await.unwrap();
            let mut lease = entry.acquire(100).unwrap();
            lease.open_request(0, true).unwrap();
            lease
                .state_mut()
                .frame
                .pending_callouts
                .push(callout("https://example.test".into(), 1000));
            let id = lease.context_id().unwrap();
            let error = resolve_callouts_on(&mut lease, id, 1000, "expired", Some(tokio::time::Instant::now()))
                .await
                .err()
                .unwrap();
            assert_eq!(error, "wasm request deadline expired");
            assert!(!lease.is_healthy());
            drop(lease);
            assert_eq!(entry.pool_len(), 0);
        },
    )
    .await;
}
