use super::*;
use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::{WafJsonConfig, WafMode};
use edgion_waf::seclang::Phase;

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
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(7));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(7));
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
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(20));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(20));
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
        .expect_ensure_request_body()
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
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(7));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(7));
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
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(7));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(7));
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
        "structure": {"maxRetainedBytes": 1048576, "maxKeyBytes": 65536, "maxMembers": 65536,
            "maxPathBytes": 16384, "maxDepth": 64, "maxWork": 16777216},
        "results": {"maxCandidates": 65536, "maxFacts": 65536, "maxRetainedBytes": 1048576,
            "maxWork": 16777216, "maxDeferredEntries": 4096, "maxDeferredBytes": 1048576}
    }))
    .unwrap()
}

#[tokio::test]
async fn full_json_default_and_explicit_limits_use_one_proven_shared_read() {
    for explicit in [false, true] {
        let mut config = super::fixture::config("SecRule ARGS:json.a \"@streq hit\" \"id:1,phase:2,pass\"\n");
        config.request_body.inspection = WafRequestBodyInspection::Full;
        config.request_body.json = explicit.then(explicit_json_config);
        let plugin = WafPlugin::try_new(&config).unwrap();
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/json",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        session.expect_ctx().return_const(EdgionHttpContext::new());
        let body = Bytes::from_static(br#"{"a":"gone","a":"hit"}"#);
        let len = body.len() as u64;
        let mut sequence = mockall::Sequence::new();
        session
            .expect_ensure_request_body()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| Ok(len));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| Ok(len));
        session
            .expect_read_request_body()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| Ok(body));
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
    }
}

#[tokio::test]
async fn full_capture_proof_and_all_caps_precede_materialization() {
    for case in 0..5 {
        let mut config = super::fixture::config("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n");
        config.request_body.inspection = WafRequestBodyInspection::Full;
        config.request_body.json = Some(explicit_json_config());
        let plugin = WafPlugin::try_new(&config).unwrap();
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/json",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = if case == 2 { 8 } else { 32 << 20 };
        session.expect_ctx().return_const(ctx);
        let len = match case {
            0 | 1 => 0,
            2 => 9,
            3 => (16 << 20) + 1,
            _ => (1 << 20) + 1,
        };
        session
            .expect_ensure_request_body()
            .times(1)
            .return_once(move |_| Ok(len));
        session
            .expect_request_body_available_len()
            .times(1)
            .return_once(move |_| match case {
                0 => Err(BodyReadError::NotBuffered),
                1 => Ok(1),
                _ => Ok(len),
            });
        session.expect_read_request_body().times(0);
        session.expect_record_error().returning(|_| ());
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::ErrResponse {
                status: if case < 2 { 500 } else { 413 },
                body: None
            }
        );
    }
}

#[tokio::test]
async fn processor_override_precedes_json_only_caps_and_raw_reads() {
    let mut config = super::fixture::config(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:requestBodyProcessor=URLENCODED\"\n",
        "SecRule ARGS:a \"@streq hit\" \"id:2,phase:2,pass\"\n"
    ));
    config.request_body.inspection = WafRequestBodyInspection::Full;
    let mut limits = explicit_json_config();
    limits.lexical.max_source_bytes = 0;
    config.request_body.json = Some(limits);
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/json",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(5));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(5));
    session
        .expect_read_request_body()
        .times(1)
        .return_once(|_| Ok(Bytes::from_static(b"a=hit")));
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
async fn shared_read_length_mismatch_fails_closed() {
    let plugin = plugin("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n", WafMode::On);
    let mut session = MockPluginSession::new();
    expect_request_head(&mut session, "/");
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(3));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(3));
    session
        .expect_read_request_body()
        .times(1)
        .return_once(|_| Ok(Bytes::new()));
    session.expect_record_error().returning(|_| ());
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn canonical_resident_spilled_and_edited_bodies_keep_the_inspected_source() {
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;
    for (memory_limit, edited) in [(1024, false), (0, false), (1024, true), (0, true)] {
        let original = Bytes::from_static(br#"{"x":"MISS"}"#);
        let (mut storage, view) = BodyStorage::new(memory_limit, 1024);
        storage.write(&original).await.unwrap();
        storage.finish().await.unwrap();
        if edited {
            view.replace_spans(view.generation(), &[(6..10, Bytes::from_static(b"HIT"))])
                .unwrap();
        }
        let body = view.read_all().await.unwrap();
        if memory_limit != 0 && !edited {
            let mut reader = view.snapshot_reader().unwrap();
            let resident = reader.next_chunk().await.unwrap().unwrap();
            assert_eq!(body.as_ptr(), resident.as_ptr());
        }
        let expected = if edited { "HIT" } else { "MISS" };
        let rules = format!(
            concat!(
                "SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=On\"\n",
                "SecRule REQUEST_BODY \"@contains {}\" \"id:2,phase:3,deny,status:409\"\n"
            ),
            expected
        );
        let plugin = plugin(&rules, WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        session.expect_ctx().return_const(EdgionHttpContext::new());
        let len = body.len() as u64;
        session
            .expect_ensure_request_body()
            .times(1)
            .return_once(move |_| Ok(len));
        session
            .expect_request_body_available_len()
            .times(1)
            .return_once(move |_| Ok(len));
        session
            .expect_read_request_body()
            .times(1)
            .return_once(move |_| Ok(body));
        session
            .expect_install_waf_transaction()
            .times(1)
            .return_once(move |_, mut tx| {
                view.replace_spans(view.generation(), &[(0..len, Bytes::from_static(b"changed"))])
                    .unwrap();
                tx.publish_response_head(ResponseHead::new(200, &[])).unwrap();
                assert_eq!(
                    tx.execute_response_headers().unwrap().intervention().unwrap().status(),
                    409
                );
                true
            });
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn only_transport_proven_empty_capture_can_be_materialized() {
    for declared in [Some(false), Some(true), None] {
        let plugin = plugin("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n", WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/");
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.body_declared = declared;
        session.expect_ctx().return_const(ctx);
        session.expect_ensure_request_body().times(1).return_once(|_| Ok(0));
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(declared == Some(false)))
            .times(1)
            .return_once(move |_| {
                if declared == Some(false) {
                    Ok(0)
                } else {
                    Err(BodyReadError::NotBuffered)
                }
            });
        if declared == Some(false) {
            session
                .expect_read_request_body()
                .times(1)
                .return_once(|_| Ok(Bytes::new()));
            session
                .expect_install_waf_transaction()
                .times(1)
                .return_once(|_, _| true);
        } else {
            session.expect_read_request_body().times(0);
            session.expect_record_error().returning(|_| ());
        }
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            if declared == Some(false) {
                PluginRunningResult::GoodNext
            } else {
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                }
            }
        );
    }
}

#[tokio::test]
async fn default_json_body_independent_rules_and_phase_one_denial_do_not_capture() {
    for deny in [false, true] {
        let rules = if deny {
            "SecRule REQUEST_URI \"@contains allowed\" \"id:1,phase:1,deny,status:403\"\n"
        } else {
            "SecRule REQUEST_URI \"@contains allowed\" \"id:1,phase:2,pass\"\n"
        };
        let plugin = plugin(rules, WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/allowed");
        session.expect_ensure_request_body().times(0);
        session.expect_read_request_body().times(0);
        if !deny {
            session
                .expect_install_waf_transaction()
                .times(1)
                .return_once(|_, _| true);
        }
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            if deny {
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
async fn json_physical_failure_still_rejects_detection_only() {
    let mut config = super::fixture::config("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n");
    config.mode = WafMode::DetectionOnly;
    config.request_body.inspection = WafRequestBodyInspection::Full;
    let mut limits = explicit_json_config();
    limits.results.max_candidates = 0;
    config.request_body.json = Some(limits);
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    expect_request_head_with_headers(
        &mut session,
        "/",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_ensure_request_body().times(1).return_once(|_| Ok(11));
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(11));
    session
        .expect_read_request_body()
        .times(1)
        .return_once(|_| Ok(Bytes::from_static(br#"{"a":"hit"}"#)));
    session.expect_install_waf_transaction().times(0);
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[test]
fn json_error_categories_preserve_status_without_exposing_payloads() {
    use edgion_waf::seclang::{JsonResultLimit, JsonStructureLimit};
    let cases = [
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                    kind: JsonStreamLimit::SourceBytes,
                    limit: 0,
                }),
            },
            413,
            WafErrorCode::BodyLimit,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                    kind: JsonStreamLimit::Depth,
                    limit: 0,
                }),
            },
            500,
            WafErrorCode::RuntimeLimit,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Limit {
                    kind: JsonStructureLimit::Work,
                    limit: 0,
                },
            },
            500,
            WafErrorCode::RuntimeLimit,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Allocation,
            },
            500,
            WafErrorCode::Allocation,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Allocation),
            },
            500,
            WafErrorCode::Allocation,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Arithmetic,
            },
            500,
            WafErrorCode::Arithmetic,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Arithmetic),
            },
            500,
            WafErrorCode::Arithmetic,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::State,
            },
            500,
            WafErrorCode::PhaseState,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::AlreadyComplete,
            },
            500,
            WafErrorCode::PhaseState,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Abandoned),
            },
            500,
            WafErrorCode::PhaseState,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Syntax),
            },
            500,
            WafErrorCode::BodyProcessing,
        ),
        (
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Sink,
            },
            500,
            WafErrorCode::BodyProcessing,
        ),
        (
            RuntimeError::JsonResultLimit {
                kind: JsonResultLimit::Records,
            },
            500,
            WafErrorCode::RuntimeLimit,
        ),
        (
            RuntimeError::JsonUnsupported { rule_id: 42 },
            500,
            WafErrorCode::Unsupported,
        ),
        (RuntimeError::JsonInspectionState, 500, WafErrorCode::RuntimeInvariant),
    ];
    let plugin = plugin("SecAction \"id:1,phase:1,pass\"\n", WafMode::On);
    for (error, status, code) in cases {
        assert_eq!(WafPlugin::runtime_error_status(&error), status);
        assert_eq!(
            std::mem::discriminant(&WafPlugin::runtime_error_code(&error)),
            std::mem::discriminant(&code)
        );
        let mut session = MockPluginSession::new();
        session
            .expect_record_waf_terminal_error()
            .times(1)
            .returning(|_, _, _, _| ());
        assert_eq!(
            plugin.runtime_failure(&mut session, WafLogPhase::RequestBody, &error),
            PluginRunningResult::ErrResponse { status, body: None }
        );
    }
}
