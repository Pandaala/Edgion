use super::super::request::WasmRequestState;
use super::super::vm_manager::ModuleEntry;
use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::plugin_configs::WasmConfig;
use parking_lot::Mutex;

fn plugin() -> Arc<WasmPlugin> {
    let mut config = WasmConfig::default();
    config.source.file = Some(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../edgion-tests/wasm/artifacts/lifecycle_demo.wasm"
        )
        .into(),
    );
    config.response = WasmResponseHooks {
        headers: true,
        body: true,
        trailers: true,
    };
    Arc::new(super::super::plugin::tests::actual_plugin(&config))
}

struct SessionState {
    owners: Arc<Mutex<WasmRequestState>>,
    body: Arc<Mutex<Vec<Box<dyn StreamingResponseProcessor>>>>,
    trailers: Arc<Mutex<Vec<Box<dyn UpstreamResponseTrailer>>>>,
}

fn session(marker: &'static str, header_eos: bool) -> (MockPluginSession, SessionState) {
    let owners = Arc::new(Mutex::new(WasmRequestState::default()));
    let body = Arc::new(Mutex::new(Vec::new()));
    let trailers = Arc::new(Mutex::new(Vec::new()));
    let mut s = MockPluginSession::new();
    let o = owners.clone();
    s.expect_install_wasm_binding()
        .returning(move |binding| o.lock().install(binding));
    let o = owners.clone();
    s.expect_wasm_binding().returning(move |id| o.lock().get(id));
    let b = body.clone();
    s.expect_install_attached_response_processor()
        .returning(move |processor| {
            b.lock().push(processor);
            true
        });
    let t = trailers.clone();
    s.expect_install_upstream_response_trailer()
        .returning(move |processor| {
            t.lock().push(processor);
            true
        });
    s.expect_request_headers()
        .returning(move || vec![("x-lifecycle-id".into(), marker.into())]);
    s.expect_get_path().return_const("/".to_string());
    s.expect_get_method().return_const("POST".to_string());
    s.expect_header_value().returning(|_| None);
    s.expect_remote_ip().return_const("127.0.0.1".to_string());
    s.expect_request_headers_end_stream().return_const(Some(false));
    s.expect_ctx().return_const(crate::EdgionHttpContext::new());
    s.expect_read_request_body()
        .returning(move |_| Ok(Bytes::from_static(marker.as_bytes())));
    s.expect_set_request_header()
        .withf(move |name, value| name == "x-context-request" && value == marker)
        .returning(|_, _| Ok(()));
    s.expect_response_headers()
        .returning(|| vec![(":status".into(), "200".into())]);
    s.expect_get_response_status().return_const(Some(200u16));
    s.expect_upstream_response_headers_end_stream()
        .return_const(Some(header_eos));
    s.expect_set_response_header()
        .withf(move |name, value| name == "x-context-response" && value == marker)
        .returning(|_, _| Ok(()));
    (s, SessionState { owners, body, trailers })
}

fn counter(entry: &ModuleEntry, key: &str) -> Option<u64> {
    entry
        .shared_data_for_test()
        .get(key)
        .map(|(value, _)| u64::from_le_bytes(value.try_into().unwrap()))
}

async fn start(plugin: &Arc<WasmPlugin>, session: &mut MockPluginSession) {
    let mut log = PluginLog::new("Wasm");
    assert_eq!(
        RequestFilter::run_request(plugin, session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        UpstreamResponse::run_upstream_response(plugin, session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn sdk_fields_survive_interleaved_requests_chunks_trailers_and_generation_retirement() {
    let plugin = plugin();
    let entry = plugin.runtime.get_or_load().await.unwrap();
    let (mut a, a_state) = session("A", false);
    let (mut b, b_state) = session("B", false);
    start(&plugin, &mut a).await;
    start(&plugin, &mut b).await;
    let mut a_body = a_state.body.lock().pop().unwrap();
    let mut b_body = b_state.body.lock().pop().unwrap();
    let mut a_trailers = a_state.trailers.lock().pop().unwrap();
    let mut b_trailers = b_state.trailers.lock().pop().unwrap();
    // A rebuilt configured generation has distinct mutable state. Old bindings
    // remain executable even after their plugin owner stops its background task.
    let replacement = self::plugin();
    let replacement_entry = replacement.runtime.get_or_load().await.unwrap();
    assert!(!Arc::ptr_eq(&entry, &replacement_entry));
    drop(plugin);
    let mut sink = ResponseBodySink::new();
    for _ in 0..3 {
        a_body
            .on_event(
                &mut Some(Bytes::from_static(b"aa")),
                StreamingResponseEvent::Data { end_of_stream: false },
                ResponseStreamKind::None,
                &mut sink,
                &mut a,
            )
            .await;
        b_body
            .on_event(
                &mut Some(Bytes::from_static(b"bbb")),
                StreamingResponseEvent::Data { end_of_stream: false },
                ResponseStreamKind::None,
                &mut sink,
                &mut b,
            )
            .await;
    }
    a_body
        .on_event(
            &mut Some(Bytes::from_static(b"tail")),
            StreamingResponseEvent::TerminalBeforeTrailers,
            ResponseStreamKind::None,
            &mut sink,
            &mut a,
        )
        .await;
    assert_eq!(counter(&entry, "A:eos"), None);
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-lifecycle-trailer", "A".parse().unwrap());
    let mut log = PluginLog::new("Wasm");
    a_trailers
        .run_upstream_response_trailer(Some(&mut trailers), &mut a, &mut log)
        .await;
    b_body
        .on_event(
            &mut None,
            StreamingResponseEvent::TerminalWithoutTrailers,
            ResponseStreamKind::None,
            &mut sink,
            &mut b,
        )
        .await;
    b_trailers.run_upstream_response_trailer(None, &mut b, &mut log).await;
    assert_eq!(counter(&entry, "A:bytes"), Some(10));
    assert_eq!(counter(&entry, "B:bytes"), Some(9));
    assert_eq!(counter(&entry, "A:chunks"), Some(4));
    assert_eq!(counter(&entry, "B:chunks"), Some(4));
    assert_eq!(counter(&entry, "A:trailers"), Some(1));
    assert_eq!(counter(&entry, "A:eos"), Some(1));
    assert_eq!(counter(&entry, "B:eos"), Some(1));
    assert!(a_state.owners.lock().finish());
    assert!(b_state.owners.lock().finish());
    assert!(a_state.owners.lock().finish());
    assert_eq!(counter(&entry, "A:done"), Some(1));
    assert_eq!(counter(&entry, "B:done"), Some(1));
    assert_eq!(counter(&replacement_entry, "A:done"), None);
}

#[tokio::test]
async fn sdk_header_eos_suppresses_duplicate_body_and_cancellation_never_calls_done() {
    let plugin = plugin();
    let entry = plugin.runtime.get_or_load().await.unwrap();
    let (mut s, state) = session("empty", true);
    start(&plugin, &mut s).await;
    let mut body = state.body.lock().pop().unwrap();
    let mut sink = ResponseBodySink::new();
    body.on_event(
        &mut None,
        StreamingResponseEvent::TerminalWithoutTrailers,
        ResponseStreamKind::None,
        &mut sink,
        &mut s,
    )
    .await;
    assert_eq!(counter(&entry, "empty:chunks"), None);
    assert_eq!(counter(&entry, "empty:eos"), Some(1));
    body.release_inflight();
    assert!(state.owners.lock().finish());
    assert_eq!(counter(&entry, "empty:done"), None);
}

#[tokio::test]
async fn sdk_timer_root_fields_survive_multiple_rounds() {
    let plugin = plugin();
    let entry = plugin.runtime.get_or_load().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if counter(&entry, "root-ticks").is_some_and(|n| n >= 3) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (mut s, state) = session("deny", false);
    let mut log = PluginLog::new("Wasm");
    assert!(matches!(
        RequestFilter::run_request(&plugin, &mut s, &mut log).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));
    assert!(state.owners.lock().finish());
    assert_eq!(counter(&entry, "deny:done"), Some(1));
}

#[tokio::test]
async fn attached_body_deadline_discards_binding_and_preserves_later_chunks() {
    let guest = wat::parse_str(
        r#"(module
          (memory (export "memory") 1)
          (func (export "proxy_on_memory_allocate") (param i32) (result i32) i32.const 1024)
          (func (export "proxy_on_context_create") (param i32 i32))
          (func (export "proxy_on_vm_start") (param i32 i32) (result i32) i32.const 1)
          (func (export "proxy_on_configure") (param i32 i32) (result i32) i32.const 1)
          (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) i32.const 0)
          (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
            (loop $busy (br $busy)) i32.const 0)
          (func (export "proxy_on_done") (param i32) (result i32) unreachable)
          (func (export "proxy_on_delete") (param i32) unreachable)
        )"#,
    )
    .unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), guest).unwrap();
    let mut config = WasmConfig {
        timeout_ms: 10_000,
        ..Default::default()
    };
    config.source.file = Some(file.path().to_string_lossy().into_owned());
    config.response.body = true;
    let plugin = Arc::new(super::super::plugin::tests::actual_plugin(&config));
    let entry = plugin.runtime.get_or_load().await.unwrap();
    let (mut s, state) = session("timeout", false);
    s.expect_record_error()
        .withf(|error| *error == EdgionErrorCode::WasmExecutionError)
        .times(1)
        .return_const(());
    start(&plugin, &mut s).await;
    let binding = state.owners.lock().get(&plugin.identity).unwrap();
    let mut processor = state.body.lock().pop().unwrap();
    let mut sink = ResponseBodySink::new();
    let mut chunk = Some(Bytes::from_static(b"first"));
    let started = std::time::Instant::now();
    processor
        .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut s)
        .await;
    // The body budget is 1s plus epoch slack, not the request's 10s.
    // Leave ample scheduling headroom while rejecting the original bug.
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(chunk.as_deref(), Some(b"first".as_slice()));
    assert!(binding.take().is_none());
    assert_eq!(entry.pool_len(), 0);

    let mut later = Some(Bytes::from_static(b"later"));
    processor
        .on_chunk(&mut later, true, ResponseStreamKind::None, &mut sink, &mut s)
        .await;
    assert_eq!(later.as_deref(), Some(b"later".as_slice()));
    assert!(state.owners.lock().finish());
    assert_eq!(entry.pool_len(), 0);
}

#[tokio::test]
async fn trailer_snapshot_bounds_preserve_guest_dispatch_and_cleanup() {
    for (fields, bytes, accepted) in [
        (1023, None, true),
        (1024, None, true),
        (1025, None, false),
        (2, Some(65535), true),
        (2, Some(65536), true),
        (2, Some(65537), false),
    ] {
        let plugin = plugin();
        let entry = plugin.runtime.get_or_load().await.unwrap();
        let (mut session, state) = session("limits", false);
        session
            .expect_record_error()
            .withf(|error| *error == EdgionErrorCode::WasmExecutionError)
            .times(usize::from(!accepted))
            .return_const(());
        start(&plugin, &mut session).await;
        let mut observer = state.trailers.lock().pop().unwrap();
        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-lifecycle-trailer", "limits".parse().unwrap());
        if let Some(bytes) = bytes {
            let base = "x-lifecycle-trailer".len() + "limits".len() + "x-pad".len();
            trailers.insert("x-pad", http::HeaderValue::from_str(&"a".repeat(bytes - base)).unwrap());
            assert_eq!(
                trailers.iter().map(|(k, v)| k.as_str().len() + v.len()).sum::<usize>(),
                bytes
            );
        } else {
            for _ in 1..fields {
                trailers.append("x-extra", "a".parse().unwrap());
            }
        }
        assert_eq!(trailers.len(), fields);
        let original = trailers.clone();
        let mut log = PluginLog::new("Wasm");
        for _ in 0..2 {
            assert!(matches!(
                observer
                    .run_upstream_response_trailer(Some(&mut trailers), &mut session, &mut log)
                    .await,
                UpstreamTrailerResult::Continue
            ));
            assert_eq!(trailers, original);
        }
        assert_eq!(counter(&entry, "limits:trailers"), accepted.then_some(1));
        assert!(state.owners.lock().finish());
        assert!(state.owners.lock().finish());
        assert_eq!(counter(&entry, "limits:done"), accepted.then_some(1));
    }
}
