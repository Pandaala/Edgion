use super::*;
use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::{WafJsonConfig, WafMode};
use edgion_waf::seclang::{EvaluationMode, Phase};

use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::EdgionHttpContext;

fn plugin(rules: &str, mode: WafMode) -> WafPlugin {
    plugin_with_inspection(rules, mode, WafRequestBodyInspection::Full)
}

fn plugin_with_inspection(rules: &str, mode: WafMode, inspection: WafRequestBodyInspection) -> WafPlugin {
    WafPlugin::try_new(&WafConfig {
        mode,
        request_body: edgion_resources::resources::edgion_plugins::WafRequestBodyConfig {
            inspection,
            ..Default::default()
        },
        ..super::fixture::config(rules)
    })
    .expect("test WAF compiles")
}

#[test]
fn dedicated_summary_suppresses_all_generic_stage_logs() {
    let plugin = plugin("SecAction \"id:1,phase:1,pass\"\n", WafMode::On);
    assert!(RequestFilter::suppress_automatic_log(&plugin));
    assert!(UpstreamResponseFilter::suppress_automatic_log(&plugin));
    assert!(UpstreamResponse::suppress_automatic_log(&plugin));
}

#[test]
fn missing_policy_ref_is_rejected_by_the_gateway_constructor() {
    let config = WafConfig::default();

    assert_eq!(
        WafPlugin::try_new(&config).err().as_deref(),
        Some("policyRef requires a valid name")
    );
}

fn expect_request_head(session: &mut MockPluginSession, uri: &str) {
    expect_request_head_with_headers(session, uri, Vec::new());
}

fn expect_request_head_with_headers(session: &mut MockPluginSession, uri: &str, headers: Vec<(Vec<u8>, Vec<u8>)>) {
    session
        .expect_claim_waf_phase()
        .returning(|_, _| WafPhaseClaim::Execute);
    session.expect_record_waf_phase_outcome().returning(|_, _, _, _| ());
    session.expect_record_waf_terminal_error().returning(|_, _, _, _| ());
    // Serve the borrowed map the plugin now reads, built from the same raw
    // bytes so a non-UTF-8 header stays visible to the WAF.
    let mut map = http::HeaderMap::new();
    for (name, value) in &headers {
        map.append(
            http::HeaderName::from_bytes(name).expect("test header name must be valid"),
            http::HeaderValue::from_bytes(value).expect("test header value must be valid"),
        );
    }
    session.expect_request_header_map().return_const(map);
    session.expect_get_method().times(1).return_const("POST".to_string());
    session.expect_get_uri().return_const(uri.to_string());
    session.expect_get_request_uri().return_const(uri.to_string());
    session.expect_get_http_protocol().return_const("HTTP/1.1".to_string());
    session.expect_remote_ip().return_const("192.0.2.1".to_string());
    session.expect_get_request_id().return_const("request-1".to_string());
}

#[tokio::test]
async fn phase_one_denial_happens_before_request_body_read() {
    let plugin = plugin(
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny,status:403\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/blocked");
    let mut log = PluginLog::new(NAME);

    let result = RequestFilter::run_request(&plugin, &mut session, &mut log).await;

    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: None
        }
    );
    assert!(!log.contains("blocked rule"));
}

#[tokio::test]
async fn header_only_generation_does_not_capture_an_unused_body() {
    let plugin = plugin(
        "SecRule REQUEST_URI \"@contains /allowed\" \"id:1,phase:1,pass\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/allowed");
    session.expect_install_waf_transaction().return_once(|_, transaction| {
        drop(transaction);
        true
    });
    let mut log = PluginLog::new(NAME);

    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn phase_two_headers_do_not_read_body_in_either_inspection_mode() {
    for inspection in [WafRequestBodyInspection::Prefix, WafRequestBodyInspection::Full] {
        for blocked in [false, true] {
            let plugin = plugin_with_inspection(
                "SecRule REQUEST_HEADERS:X-Test \"@streq blocked\" \"id:1,phase:2,deny,status:409\"\n",
                WafMode::On,
                inspection,
            );
            assert!(!plugin.shared.requires_request_body);
            let mut session = MockPluginSession::new();
            expect_request_head_with_headers(
                &mut session,
                "/",
                vec![(
                    b"X-Test".to_vec(),
                    if blocked {
                        b"blocked".to_vec()
                    } else {
                        b"allowed".to_vec()
                    },
                )],
            );
            session.expect_read_request_body().times(0);
            session.expect_read_request_body_prefix().times(0);
            if !blocked {
                session
                    .expect_install_waf_transaction()
                    .times(1)
                    .return_once(|_, transaction| {
                        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
                        true
                    });
            }
            let result = RequestFilter::run_request(&plugin, &mut session, &mut PluginLog::new(NAME)).await;
            assert_eq!(
                result,
                if blocked {
                    PluginRunningResult::ErrResponse {
                        status: 409,
                        body: None,
                    }
                } else {
                    PluginRunningResult::GoodNext
                }
            );
        }
    }
}

#[tokio::test]
async fn raw_phrase_prefix_is_decided_before_returning_to_dispatch() {
    let plugin = plugin_with_inspection(
        "SecRule REQUEST_BODY \"@pm blocked\" \"id:20,phase:2,deny,status:409\"\n",
        WafMode::On,
        WafRequestBodyInspection::Prefix,
    );
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/stream");
    session.expect_install_waf_transaction().return_once(|_, transaction| {
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
        drop(transaction);
        true
    });
    session
        .expect_read_request_body_prefix()
        .withf(|limit| *limit == edgion_waf::seclang::DEFAULT_REQUEST_BODY_PREFIX_BYTES)
        .return_once(|_| Ok((Bytes::from_static(b"allowed"), true)));
    let mut log = PluginLog::new(NAME);

    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn configured_prefix_size_reaches_capture_and_engine() {
    let plugin = WafPlugin::try_new(&WafConfig {
        request_body: edgion_resources::resources::edgion_plugins::WafRequestBodyConfig {
            prefix_size: Some("1m".to_string()),
            ..Default::default()
        },
        ..super::fixture::config("SecRule REQUEST_BODY \"@contains blocked\" \"id:21,phase:2,deny,status:409\"")
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/prefix");
    session
        .expect_read_request_body_prefix()
        .withf(|limit| *limit == 1024 * 1024)
        .times(1)
        .return_once(|limit| {
            let mut body = vec![b'x'; limit];
            body[limit - 7..].copy_from_slice(b"blocked");
            Ok((Bytes::from(body), false))
        });
    let mut log = PluginLog::new(NAME);
    assert!(matches!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 409, .. }
    ));
}

#[tokio::test]
async fn prefix_match_blocks_before_dispatch_and_detection_only_records() {
    for mode in [WafMode::On, WafMode::DetectionOnly] {
        let plugin = plugin_with_inspection(
            "SecRule REQUEST_BODY \"@contains blocked\" \"id:21,phase:2,deny,status:409\"\n",
            mode,
            WafRequestBodyInspection::Prefix,
        );
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/prefix");
        session
            .expect_read_request_body_prefix()
            .times(1)
            .return_once(|_| Ok((Bytes::from_static(b"blocked"), true)));
        if mode == WafMode::DetectionOnly {
            session.expect_install_waf_transaction().return_once(|_, transaction| {
                assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
                true
            });
        }
        let result = RequestFilter::run_request(&plugin, &mut session, &mut PluginLog::new(NAME)).await;
        assert_eq!(
            result,
            if mode == WafMode::On {
                PluginRunningResult::ErrResponse {
                    status: 409,
                    body: None,
                }
            } else {
                PluginRunningResult::GoodNext
            }
        );
    }
}

#[tokio::test]
async fn full_mode_captures_body_for_later_phase_only_dependency() {
    let plugin = plugin(
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:22,phase:3,deny,status:409\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/full");
    session.expect_ctx().return_const(EdgionHttpContext::default());
    session
        .expect_read_request_body()
        .times(1)
        .return_once(|_| Ok(Bytes::from_static(b"blocked")));
    session
        .expect_install_waf_transaction()
        .return_once(|_, mut transaction| {
            transaction.publish_response_head(ResponseHead::new(200, &[])).unwrap();
            assert_eq!(
                transaction
                    .execute_response_headers()
                    .unwrap()
                    .intervention()
                    .unwrap()
                    .status(),
                409
            );
            true
        });
    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn duplicate_request_headers_are_preserved_for_phase_one() {
    let plugin = plugin(
        "SecRule REQUEST_HEADERS:X-Test \"@streq blocked\" \"id:3,phase:1,deny,status:409\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/headers",
        vec![
            (b"x-test".to_vec(), b"allowed".to_vec()),
            (b"x-test".to_vec(), b"blocked".to_vec()),
        ],
    );
    let mut log = PluginLog::new(NAME);

    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 409,
            body: None
        }
    );
}

#[tokio::test]
async fn duplicate_response_headers_are_preserved_for_phase_three() {
    let plugin = plugin(
        "SecRule RESPONSE_HEADERS:X-Test \"@streq blocked\" \"id:30,phase:3,deny,status:409\"\n",
        WafMode::On,
    );
    let slot = Arc::new(parking_lot::Mutex::new(None));
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/headers");
    let install_slot = Arc::clone(&slot);
    session
        .expect_install_waf_transaction()
        .return_once(move |_, transaction| {
            *install_slot.lock() = Some(transaction);
            true
        });
    let take_slot = Arc::clone(&slot);
    session
        .expect_take_waf_transaction()
        .return_once(move |_| take_slot.lock().take());
    let mut response_headers = http::HeaderMap::new();
    response_headers.append("x-test", http::HeaderValue::from_static("allowed"));
    response_headers.append("x-test", http::HeaderValue::from_static("blocked"));
    session.expect_response_header_map().return_const(response_headers);
    session.expect_get_response_status().return_const(Some(200_u16));

    let mut request_log = PluginLog::new(NAME);
    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut request_log).await,
        PluginRunningResult::GoodNext
    );

    let mut response_log = PluginLog::new(NAME);
    assert_eq!(
        UpstreamResponseFilter::run_upstream_response_filter(&plugin, &mut session, &mut response_log),
        PluginRunningResult::ErrResponse {
            status: 409,
            body: None
        }
    );
}

#[tokio::test]
async fn malformed_structured_body_fails_closed_without_payload_logging() {
    let plugin = plugin("SecRule ARGS \"@contains value\" \"id:4,phase:2,pass\"\n", WafMode::On);
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/json",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 1024;
    session.expect_ctx().return_const(ctx);
    session
        .expect_read_request_body()
        .return_once(|_| Ok(Bytes::from_static(b"private-invalid-json")));
    let mut log = PluginLog::new(NAME);

    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
    assert!(!log.contains("private"));
}

#[tokio::test]
async fn gateway_body_cap_failure_maps_to_413() {
    use crate::plugins::runtime::traits::session::BodyReadError;

    let plugin = plugin(
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:5,phase:2,pass\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/large");
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 8;
    session.expect_ctx().return_const(ctx);
    session
        .expect_read_request_body()
        .return_once(|_| Err(BodyReadError::BufferLimitExceeded { limit: 8 }));
    session.expect_record_error().returning(|_| ());
    let mut log = PluginLog::new(NAME);

    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn detection_only_records_body_match_and_keeps_the_transaction() {
    let plugin = plugin(
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,deny,status:406\"\n",
        WafMode::DetectionOnly,
    );
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/submit");
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 1024;
    session.expect_ctx().return_const(ctx);
    session
        .expect_read_request_body()
        .withf(|limit| *limit == Some(1024))
        .return_once(|_| Ok(Bytes::from_static(b"blocked")));
    session
        .expect_install_waf_transaction()
        .times(1)
        .return_once(|_, transaction| {
            drop(transaction);
            true
        });
    let mut log = PluginLog::new(NAME);

    let result = RequestFilter::run_request(&plugin, &mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(!log.contains("matched"));
    assert!(!log.contains("blocked rule"));
}

#[tokio::test]
async fn one_transaction_advances_through_all_gateway_phase_hooks() {
    let plugin = plugin(
        concat!(
            "SecRule REQUEST_BODY \"@contains allowed\" \"id:10,phase:2,pass\"\n",
            "SecRule RESPONSE_STATUS \"@streq 201\" \"id:11,phase:3,pass\"\n",
            "SecAction \"id:12,phase:4,pass\"\n",
            "SecAction \"id:13,phase:5,pass\"\n"
        ),
        WafMode::On,
    );
    let slot = Arc::new(parking_lot::Mutex::new(None));
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/submit");
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 1024;
    session.expect_ctx().return_const(ctx);
    session
        .expect_read_request_body()
        .return_once(|_| Ok(Bytes::from_static(b"allowed")));
    let install_slot = Arc::clone(&slot);
    session
        .expect_install_waf_transaction()
        .times(2)
        .returning(move |_, transaction| {
            let mut guard = install_slot.lock();
            if guard.is_some() {
                return false;
            }
            *guard = Some(transaction);
            true
        });
    let take_slot = Arc::clone(&slot);
    session
        .expect_take_waf_transaction()
        .times(2)
        .returning(move |_| take_slot.lock().take());
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::new());
    session.expect_get_response_status().return_const(Some(201_u16));

    let mut request_log = PluginLog::new(NAME);
    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut request_log).await,
        PluginRunningResult::GoodNext
    );
    let mut header_log = PluginLog::new(NAME);
    assert_eq!(
        UpstreamResponseFilter::run_upstream_response_filter(&plugin, &mut session, &mut header_log),
        PluginRunningResult::GoodNext
    );
    let mut response_log = PluginLog::new(NAME);
    assert_eq!(
        UpstreamResponse::run_upstream_response(&plugin, &mut session, &mut response_log).await,
        PluginRunningResult::GoodNext
    );
    assert!(slot.lock().is_none());
}

fn explicit_json_config() -> WafJsonConfig {
    serde_json::from_value(serde_json::json!({
        "lexical": {"maxSourceBytes": 1048576, "maxRawTokenBytes": 65536, "maxDecodedBytes": 1048576,
            "maxEvents": 65536, "maxDepth": 64, "maxRetainedBytes": 1048576, "maxWork": 16777216},
        "document": {"maxRetainedBytes": 1048576, "maxKeyBytes": 65536, "maxMembers": 65536,
            "maxEvents": 65536, "maxPathBytes": 16384, "maxDepth": 64, "maxWork": 16777216},
        "plan": {"maxSubscribers": 65536, "maxRetainedBytes": 1048576},
        "facts": {"maxFacts": 65536, "maxRetainedBytes": 1048576, "maxWork": 16777216}
    }))
    .unwrap()
}

fn json_plugin(rules: &str, mode: WafMode) -> WafPlugin {
    let mut config = super::fixture::config(rules);
    config.mode = mode;
    config.request_body.inspection = WafRequestBodyInspection::Full;
    config.request_body.json = Some(explicit_json_config());
    WafPlugin::try_new(&config).unwrap()
}

fn json_transaction(plugin: &WafPlugin, uri: &[u8]) -> Transaction<'static> {
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    let mut transaction = Transaction::new_shared_with_modes(
        Arc::clone(&plugin.shared.generation),
        RequestHead::new(
            b"POST / HTTP/1.1",
            b"POST",
            uri,
            uri,
            b"HTTP/1.1",
            b"192.0.2.1",
            b"test",
            &headers,
        ),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        plugin.shared.engine_mode,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    transaction
}

#[tokio::test]
async fn json_opt_in_keeps_body_independent_generations_and_phase_one_denial_capture_free() {
    for rules in [
        "SecRule REQUEST_URI \"@contains allowed\" \"id:1,phase:2,pass\"\n",
        "SecRule REQUEST_URI \"@contains allowed\" \"id:1,phase:1,deny,status:403\"\n",
    ] {
        let plugin = json_plugin(rules, WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/allowed");
        if !rules.contains("deny") {
            session.expect_install_waf_transaction().return_once(|_, _| true);
        }
        let result = plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await;
        assert_eq!(
            result,
            if rules.contains("deny") {
                PluginRunningResult::ErrResponse {
                    status: 403,
                    body: None,
                }
            } else {
                PluginRunningResult::GoodNext
            }
        );
    }
}

#[tokio::test]
async fn json_opt_in_uses_canonical_capture_snapshot_and_installs_one_transaction() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let plugin = json_plugin(
        "SecRule ARGS:json.a \"@pm allowed\" \"id:1,phase:2,pass\"\n",
        WafMode::On,
    );
    let (mut storage, view) = BodyStorage::new(1, 1024);
    let body = Bytes::from_static(br#"{"a":"allowed"}"#);
    storage.write(&body).await.unwrap();
    storage.finish().await.unwrap();
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 1024;
    ctx.request_info.body_declared = Some(true);
    session.expect_ctx().return_const(ctx);
    session
        .expect_ensure_request_body()
        .with(mockall::predicate::eq(Some(1024)))
        .times(1)
        .return_once(move |_| Ok(body.len() as u64));
    let reader = view.snapshot_reader().unwrap();
    session
        .expect_request_body_snapshot_reader()
        .with(mockall::predicate::eq(false))
        .times(1)
        .return_once(move |_| Ok(reader));
    session
        .expect_install_waf_transaction()
        .times(1)
        .return_once(|_, transaction| {
            assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
            true
        });
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::GoodNext
    );
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn only_initial_unsupported_json_admission_uses_ordinary_full_publication() {
    let plugin = json_plugin(
        "SecRule REQUEST_BODY \"@contains allowed\" \"id:1,phase:2,pass\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_read_request_body()
        .times(1)
        .return_once(|_| Ok(Bytes::from_static(br#"{"a":"allowed"}"#)));
    session
        .expect_install_waf_transaction()
        .times(1)
        .return_once(|_, _| true);
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn snapshot_driver_matches_complete_evaluation_for_memory_spill_and_logical_edits() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    let plugin = json_plugin(
        concat!(
            "SecRule ARGS:json.a \"@pm blocked\" \"id:1,phase:2,deny,status:403\"\n",
            "SecRule ARGS \"!@pm blocked\" \"id:2,phase:2,pass\"\n",
            "SecRule ARGS_NAMES \"@pm json.a\" \"id:3,phase:2,pass\"\n",
        ),
        WafMode::On,
    );
    let body = Bytes::from_static(br#"{"a":"blocked","a":"allowed","array":[1,true,null]}"#);
    for memory_limit in [1, 65536] {
        let (mut storage, view) = BodyStorage::new(memory_limit, 1024);
        for chunk in body.chunks(3) {
            storage.write(&Bytes::copy_from_slice(chunk)).await.unwrap();
        }
        storage.finish().await.unwrap();
        // Logical segments exercise boundaries independently of transport chunks.
        view.prepend(Bytes::from_static(b" ")).unwrap();
        view.append(Bytes::from_static(b" ")).unwrap();
        let mut actual = json_transaction(&plugin, b"/?json.a=query&other=blocked");
        let mut expected = json_transaction(&plugin, b"/?json.a=query&other=blocked");
        actual
            .start_json_request_body(BodyProcessorSelection::Auto, explicit_json_config().execution_limits())
            .unwrap();
        let mut reader = view.snapshot_reader().unwrap();
        publish_json_snapshot(&mut actual, reader.as_mut(), view.len().unwrap(), 1024, 1024, 1024)
            .await
            .unwrap();
        let actual_outcome = actual.finish_json_request_body().unwrap();
        expected
            .publish_request_body(RequestBody::new(&body, BodyProcessorSelection::Auto))
            .unwrap();
        assert_eq!(actual_outcome, expected.execute_request_body().unwrap());
        assert!(!view.is_materialized());
        // Full inspection preserves subsequent canonical mutation semantics.
        view.append(Bytes::from_static(b" ")).unwrap();
    }
}

#[tokio::test]
async fn snapshot_driver_defers_malformed_mode_handling_to_canonical_engine() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    for mode in [WafMode::On, WafMode::DetectionOnly] {
        let plugin = json_plugin("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,deny\"\n", mode);
        let (mut storage, view) = BodyStorage::new(1, 1024);
        let body = Bytes::from_static(br#"{"a":"blocked",invalid}"#);
        storage.write(&body).await.unwrap();
        storage.finish().await.unwrap();
        let mut actual = json_transaction(&plugin, b"/?q=blocked");
        actual
            .start_json_request_body(BodyProcessorSelection::Auto, explicit_json_config().execution_limits())
            .unwrap();
        let mut reader = view.snapshot_reader().unwrap();
        let publish = publish_json_snapshot(&mut actual, reader.as_mut(), body.len() as u64, 1024, 1024, 1024).await;
        if mode == WafMode::On {
            assert!(publish.is_err());
            assert!(actual.finish_json_request_body().is_err());
        } else {
            publish.unwrap();
            assert!(actual.finish_json_request_body().unwrap().intervention().is_none());
            assert!(actual.request_body_error().is_some());
        }
    }
}

struct ScriptedJsonReader {
    len: u64,
    chunks: std::collections::VecDeque<Result<Option<Bytes>, BodyReadError>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl RequestBodySnapshotReader for ScriptedJsonReader {
    fn len(&self) -> u64 {
        self.len
    }
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.chunks.pop_front().unwrap_or(Ok(None))
    }
}

#[tokio::test]
async fn snapshot_driver_requires_real_eof_and_rejects_inconsistent_readers() {
    let plugin = json_plugin("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,pass\"\n", WafMode::On);
    let body = Bytes::from_static(br#"{"a":"allowed"}"#);
    let len = body.len() as u64;
    let cases = [
        (len, vec![Ok(Some(body.clone())), Ok(None)], true, 2),
        (len + 1, vec![Ok(Some(body.clone())), Ok(None)], false, 0),
        (len, vec![Ok(None)], false, 1),
        (len, vec![Ok(Some(Bytes::new()))], false, 1),
        (
            len,
            vec![Ok(Some(body.clone())), Ok(Some(Bytes::from_static(b" ")))],
            false,
            2,
        ),
        (
            len,
            vec![Ok(Some(body.clone())), Err(BodyReadError::StorageFailure)],
            false,
            2,
        ),
        (len, vec![Err(BodyReadError::Incomplete)], false, 1),
    ];
    for (metadata_len, chunks, success, expected_calls) in cases {
        let mut transaction = json_transaction(&plugin, b"/");
        transaction
            .start_json_request_body(BodyProcessorSelection::Auto, explicit_json_config().execution_limits())
            .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = ScriptedJsonReader {
            len: metadata_len,
            chunks: chunks.into(),
            calls: calls.clone(),
        };
        let result = publish_json_snapshot(&mut transaction, &mut reader, len, 1024, 1024, 1024).await;
        assert_eq!(result.is_ok(), success, "{result:?}");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), expected_calls);
        if success {
            assert!(transaction.finish_json_request_body().is_ok());
        } else {
            assert!(transaction.finish_json_request_body().is_err());
        }
    }
}

#[tokio::test]
async fn independent_json_limits_fail_closed_in_detection_only_and_before_capture() {
    let mut config = super::fixture::config("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,pass\"\n");
    config.mode = WafMode::DetectionOnly;
    config.request_body.inspection = WafRequestBodyInspection::Full;
    let mut json = explicit_json_config();
    json.plan.max_subscribers = 0;
    config.request_body.json = Some(json);
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
    let plugin = json_plugin(
        "SecRule ARGS \"@pm blocked\" \"id:1,phase:2,pass\"\n",
        WafMode::DetectionOnly,
    );
    for group in ["lexical", "document", "facts"] {
        let mut limits = explicit_json_config().execution_limits();
        match group {
            "lexical" => limits.lexical.max_work = 0,
            "document" => limits.document.max_work = 0,
            _ => limits.facts.max_facts = 0,
        }
        let mut transaction = json_transaction(&plugin, b"/");
        transaction
            .start_json_request_body(BodyProcessorSelection::Auto, limits)
            .unwrap();
        let body = Bytes::from_static(br#"{"a":"blocked"}"#);
        let mut reader = ScriptedJsonReader {
            len: body.len() as u64,
            chunks: vec![Ok(Some(body))].into(),
            calls: Default::default(),
        };
        let expected_len = reader.len;
        let error = publish_json_snapshot(&mut transaction, &mut reader, expected_len, 1024, 1024, 1024)
            .await
            .unwrap_err();
        let JsonSnapshotError::Engine(error) = error else {
            panic!("engine limit expected")
        };
        assert_json_classification(&error, 500, WafErrorCode::RuntimeLimit);
        assert!(transaction.finish_json_request_body().is_err());
    }
}

#[test]
fn json_source_length_limits_preserve_status_and_canonical_classification() {
    assert!(check_json_source_len(4, 4, 4, 4).is_ok());
    assert!(matches!(
        check_json_source_len(5, 4, 5, 5),
        Err(JsonSnapshotError::Read(BodyReadError::BufferLimitExceeded { limit: 4 }))
    ));
    for error in [
        check_json_source_len(5, 5, 4, 5).unwrap_err(),
        check_json_source_len(5, 5, 5, 4).unwrap_err(),
    ] {
        let JsonSnapshotError::Engine(error) = error else {
            panic!("engine byte cap expected")
        };
        assert_json_classification(&error, 413, WafErrorCode::BodyLimit);
    }
    assert!(check_json_source_len(0, 0, 0, 0).is_ok());
    assert!(check_json_source_len(1, 1, 1, 0).is_err());
}

#[tokio::test]
async fn bodyless_json_snapshot_completes_without_fabricating_a_data_chunk() {
    let plugin = json_plugin("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,pass\"\n", WafMode::On);
    let mut transaction = json_transaction(&plugin, b"/");
    transaction
        .start_json_request_body(BodyProcessorSelection::Auto, explicit_json_config().execution_limits())
        .unwrap();
    let state = crate::request_body::RequestBodyState::default();
    assert!(state.snapshot_reader(false).is_err());
    let mut reader = state.snapshot_reader(true).unwrap();
    publish_json_snapshot(&mut transaction, reader.as_mut(), 0, 0, 0, 0)
        .await
        .unwrap();
    assert!(transaction.finish_json_request_body().is_ok());
}

struct PendingJsonReader {
    body: Option<Bytes>,
    len: u64,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for PendingJsonReader {
    fn drop(&mut self) {
        self.dropped.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
#[async_trait]
impl RequestBodySnapshotReader for PendingJsonReader {
    fn len(&self) -> u64 {
        self.len
    }
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        if let Some(body) = self.body.take() {
            return Ok(Some(body));
        }
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_drops_pinned_transaction_and_reader_before_phase_two() {
    use std::future::Future;
    use std::task::Poll;
    let plugin = json_plugin("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,deny\"\n", WafMode::On);
    let count = Arc::strong_count(&plugin.shared.generation);
    let mut transaction = json_transaction(&plugin, b"/");
    transaction
        .start_json_request_body(BodyProcessorSelection::Auto, explicit_json_config().execution_limits())
        .unwrap();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let body = Bytes::from_static(br#"{"a":"blocked"}"#);
    let len = body.len() as u64;
    let mut reader = PendingJsonReader {
        body: Some(body),
        len,
        dropped: Arc::clone(&dropped),
    };
    let mut operation = Box::pin(async move {
        publish_json_snapshot(&mut transaction, &mut reader, len, 1024, 1024, 1024)
            .await
            .unwrap();
        transaction.finish_json_request_body().unwrap()
    });
    std::future::poll_fn(|cx| {
        assert!(
            operation.as_mut().poll(cx).is_pending(),
            "await actual EOF even after all expected bytes"
        );
        Poll::Ready(())
    })
    .await;
    assert!(Arc::strong_count(&plugin.shared.generation) > count);
    assert!(!dropped.load(std::sync::atomic::Ordering::SeqCst));
    drop(operation);
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(Arc::strong_count(&plugin.shared.generation), count);
}

#[tokio::test]
async fn json_cached_length_is_rechecked_before_snapshot_reads() {
    for (physical_cap, captured_len) in [(8, 9), (33554432, 16777217), (33554432, 1048577)] {
        let plugin = json_plugin("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,pass\"\n", WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = physical_cap;
        session.expect_ctx().return_const(ctx);
        session
            .expect_ensure_request_body()
            .with(mockall::predicate::eq(Some(physical_cap)))
            .times(1)
            .return_once(move |_| Ok(captured_len));
        session.expect_record_error().returning(|_| ());
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::ErrResponse {
                status: 413,
                body: None
            }
        );
    }
}

#[tokio::test]
async fn json_snapshot_bodyless_hint_uses_only_original_transport_metadata() {
    for body_declared in [Some(false), Some(true), None] {
        let plugin = json_plugin("SecRule ARGS \"@pm blocked\" \"id:1,phase:2,pass\"\n", WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.body_declared = body_declared;
        session.expect_ctx().return_const(ctx);
        session.expect_ensure_request_body().times(1).return_once(|_| Ok(0));
        let reader = crate::request_body::RequestBodyState::default()
            .snapshot_reader(true)
            .unwrap();
        session
            .expect_request_body_snapshot_reader()
            .with(mockall::predicate::eq(body_declared == Some(false)))
            .times(1)
            .return_once(move |_| Ok(reader));
        session
            .expect_install_waf_transaction()
            .times(1)
            .return_once(|_, _| true);
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::GoodNext
        );
    }
}

#[test]
fn json_error_mapping_is_payload_free_and_preserves_independent_error_classes() {
    use edgion_waf::seclang::{JsonDocumentLimit, JsonExecutionLimit};
    let cases = [
        (JsonExecutionError::Unsupported, 500, WafErrorCode::Unsupported),
        (JsonExecutionError::State, 500, WafErrorCode::PhaseState),
        (JsonExecutionError::AlreadyComplete, 500, WafErrorCode::PhaseState),
        (JsonExecutionError::Allocation, 500, WafErrorCode::Allocation),
        (JsonExecutionError::Arithmetic, 500, WafErrorCode::Arithmetic),
        (
            JsonExecutionError::Limit {
                kind: JsonExecutionLimit::Facts,
                limit: 0,
            },
            500,
            WafErrorCode::RuntimeLimit,
        ),
        (
            JsonExecutionError::Body(JsonBodyError::Document(JsonDocumentError::Limit {
                kind: JsonDocumentLimit::Events,
                limit: 0,
            })),
            500,
            WafErrorCode::RuntimeLimit,
        ),
        (
            JsonExecutionError::Body(JsonBodyError::Document(JsonDocumentError::Lexical(
                JsonStreamError::Limit {
                    kind: JsonStreamLimit::SourceBytes,
                    limit: 0,
                },
            ))),
            413,
            WafErrorCode::BodyLimit,
        ),
        (
            JsonExecutionError::Body(JsonBodyError::Document(JsonDocumentError::Allocation)),
            500,
            WafErrorCode::Allocation,
        ),
        (
            JsonExecutionError::Body(JsonBodyError::Document(JsonDocumentError::Lexical(
                JsonStreamError::Allocation,
            ))),
            500,
            WafErrorCode::Allocation,
        ),
        (
            JsonExecutionError::Body(JsonBodyError::Document(JsonDocumentError::Arithmetic)),
            500,
            WafErrorCode::Arithmetic,
        ),
        (
            JsonExecutionError::Body(JsonBodyError::Document(JsonDocumentError::Lexical(
                JsonStreamError::Arithmetic,
            ))),
            500,
            WafErrorCode::Arithmetic,
        ),
    ];
    for (error, status, code) in cases {
        assert_json_classification(&error, status, code);
    }
}

fn assert_json_classification(error: &JsonExecutionError, status: u16, code: WafErrorCode) {
    let actual = WafPlugin::json_error_classification(error);
    assert_eq!(actual.0, status);
    assert_eq!(std::mem::discriminant(&actual.1), std::mem::discriminant(&code));
}

#[tokio::test]
async fn json_snapshot_split_escape_and_duplicate_boundaries_preserve_ordered_phase_two() {
    let plugin = json_plugin(
        "SecRule ARGS:json.a \"@pm blocked\" \"id:1,phase:2,deny,status:403\"\n",
        WafMode::On,
    );
    for body in [
        br#"{"a":"blo\u0063ked","a":"allowed"}"#.as_slice(),
        br#"{"a":"allowed","a":"blo\u0063ked"}"#.as_slice(),
    ] {
        for chunk_size in [1, 2, 7] {
            let chunks = body
                .chunks(chunk_size)
                .map(|chunk| Ok(Some(Bytes::copy_from_slice(chunk))))
                .collect();
            let mut reader = ScriptedJsonReader {
                len: body.len() as u64,
                chunks,
                calls: Default::default(),
            };
            let mut actual = json_transaction(&plugin, b"/?json.a=query");
            let mut expected = json_transaction(&plugin, b"/?json.a=query");
            actual
                .start_json_request_body(BodyProcessorSelection::Auto, explicit_json_config().execution_limits())
                .unwrap();
            publish_json_snapshot(&mut actual, &mut reader, body.len() as u64, 1024, 1024, 1024)
                .await
                .unwrap();
            expected
                .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Auto))
                .unwrap();
            assert_eq!(
                actual.finish_json_request_body().unwrap(),
                expected.execute_request_body().unwrap()
            );
        }
    }
}
