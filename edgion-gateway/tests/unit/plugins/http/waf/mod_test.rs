use super::*;
use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::{WafJsonConfig, WafMode};
use edgion_waf::seclang::Phase;
use pingora_core::protocols::http::RequestBodyBuffer;

use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::waf_session::{MockWafSessionCapability, WafSessionCapability};
use crate::EdgionHttpContext;

fn set_partial_capture_policy(ctx: &mut EdgionHttpContext, plugin: &WafPlugin, allowed: bool) {
    assert!(ctx.select_waf(Some(crate::ctx::WafSelection::new(
        plugin.owner(),
        crate::ctx::WafLogSource::Route,
        0,
        crate::ctx::WafLogMode::On,
    ))));
    ctx.set_waf_partial_capture_allowed(allowed);
}

fn plugin(rules: &str, mode: WafMode) -> WafPlugin {
    plugin_with_inspection(rules, mode, WafRequestBodyInspection::default())
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

fn expect_request_head(session: &mut MockPluginSession, waf: &mut MockWafSessionCapability, uri: &str) {
    expect_request_head_with_headers(session, waf, uri, Vec::new());
}

fn expect_request_head_with_headers(
    session: &mut MockPluginSession,
    waf: &mut MockWafSessionCapability,
    uri: &str,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
) {
    expect_request_head_with_context(session, waf, uri, headers, EdgionHttpContext::default());
}

fn expect_request_head_with_context(
    session: &mut MockPluginSession,
    waf: &mut MockWafSessionCapability,
    uri: &str,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    ctx: EdgionHttpContext,
) {
    expect_request_head_with_identity(session, waf, uri, headers, ctx, Some("request-1"));
}

fn expect_request_head_with_identity(
    session: &mut MockPluginSession,
    waf: &mut MockWafSessionCapability,
    uri: &str,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    ctx: EdgionHttpContext,
    request_id: Option<&'static str>,
) {
    session.expect_ctx().times(1).return_const(ctx);
    waf.expect_claim_phase().returning(|_, _| WafPhaseClaim::Execute);
    waf.expect_record_phase_outcome().returning(|_, _, _, _| ());
    waf.expect_record_terminal_error().returning(|_, _, _, _| ());
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
    session.expect_get_http_protocol().return_const("HTTP/1.1");
    session.expect_remote_ip().return_const("192.0.2.1".to_string());
    session.expect_get_request_id_ref().returning(move || request_id);
}

#[tokio::test]
async fn unique_id_uses_canonical_identity_or_empty_when_missing() {
    for (identity, operator) in [
        (
            Some("0123456789abcdef0123456789abcdef"),
            "@streq 0123456789abcdef0123456789abcdef",
        ),
        (None, "@rx ^$"),
    ] {
        let plugin = plugin(
            &format!("SecRule UNIQUE_ID \"{operator}\" \"id:1,phase:1,deny,status:403\"\n"),
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        expect_request_head_with_identity(
            &mut session,
            &mut waf,
            "/identity",
            Vec::new(),
            EdgionHttpContext::default(),
            identity,
        );
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        let mut log = PluginLog::new(NAME);

        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None
            },
            "identity: {identity:?}",
        );
    }
}

#[tokio::test]
async fn phase_one_denial_happens_before_request_body_read() {
    let plugin = plugin(
        "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny,status:403\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/blocked");
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
async fn request_line_keeps_raw_entry_target_after_uri_rewrite() {
    for protocol in ["HTTP/1.1", "HTTP/2.0"] {
        let plugin = plugin(
            &format!(
                "SecRule REQUEST_LINE \"@streq POST /%61dmin?entry=%26 {protocol}\" \
                 \"id:1,phase:1,deny,status:403,chain\"\n\
                 SecRule REQUEST_URI_RAW \"@streq /%61dmin?entry=%26\" \"chain\"\n\
                 SecRule REQUEST_URI \"@streq /rewritten?current=+\" \"\"\n"
            ),
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        waf.expect_claim_phase().returning(|_, _| WafPhaseClaim::Execute);
        waf.expect_record_phase_outcome().returning(|_, _, _, _| ());
        session.expect_ctx().times(1).return_const(EdgionHttpContext::default());
        session.expect_request_header_map().return_const(http::HeaderMap::new());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_uri()
            .return_const("/rewritten?current=+".to_string());
        session
            .expect_get_request_uri()
            .return_const("/%61dmin?entry=%26".to_string());
        session.expect_get_http_protocol().return_const(protocol);
        session
            .expect_request_uri_authority()
            .times(usize::from(protocol == "HTTP/2.0"))
            .returning(|| {
                static AUTHORITY: std::sync::LazyLock<http::uri::Authority> =
                    std::sync::LazyLock::new(|| "example.test".parse().unwrap());
                Some(&AUTHORITY)
            });
        session.expect_remote_ip().return_const("192.0.2.1".to_string());
        session.expect_get_request_id_ref().returning(|| Some("request-1"));
        session.expect_ensure_request_body().never();
        session.expect_read_request_body().never();
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None,
            },
            "{protocol}",
        );
    }
}

#[tokio::test]
async fn header_only_generation_does_not_capture_an_unused_body() {
    let plugin = plugin(
        "SecRule REQUEST_URI \"@contains /allowed\" \"id:1,phase:1,pass\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/allowed");
    waf.expect_install_transaction().return_once(|_, transaction| {
        drop(transaction);
        true
    });
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
            let mut waf = MockWafSessionCapability::new();
            expect_request_head_with_headers(
                &mut session,
                &mut waf,
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
                waf.expect_install_transaction().times(1).return_once(|_, transaction| {
                    assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
                    true
                });
            }
            session
                .expect_waf()
                .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/stream");
    waf.expect_install_transaction().return_once(|_, transaction| {
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
        drop(transaction);
        true
    });
    session
        .expect_read_request_body_prefix()
        .withf(|limit| *limit == edgion_waf::seclang::DEFAULT_REQUEST_BODY_PREFIX_BYTES)
        .return_once(|_| Ok((Bytes::from_static(b"allowed"), true)));
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
    assert_eq!(
        RequestFilter::run_request(&plugin, &mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn configured_prefix_size_reaches_capture_and_engine() {
    let plugin = WafPlugin::try_new(&WafConfig {
        request_body: edgion_resources::resources::edgion_plugins::WafRequestBodyConfig {
            inspection: WafRequestBodyInspection::Prefix,
            prefix_size: Some("1m".to_string()),
            ..Default::default()
        },
        ..super::fixture::config("SecRule REQUEST_BODY \"@contains blocked\" \"id:21,phase:2,deny,status:409\"")
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/prefix");
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
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
        let mut waf = MockWafSessionCapability::new();
        expect_request_head(&mut session, &mut waf, "/prefix");
        session
            .expect_read_request_body_prefix()
            .times(1)
            .return_once(|_| Ok((Bytes::from_static(b"blocked"), true)));
        if mode == WafMode::DetectionOnly {
            waf.expect_install_transaction().return_once(|_, transaction| {
                assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
                true
            });
        }
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/full");
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
    waf.expect_install_transaction().return_once(|_, mut transaction| {
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
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head_with_headers(
        &mut session,
        &mut waf,
        "/headers",
        vec![
            (b"x-test".to_vec(), b"allowed".to_vec()),
            (b"x-test".to_vec(), b"blocked".to_vec()),
        ],
    );
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/headers");
    let install_slot = Arc::clone(&slot);
    waf.expect_install_transaction().return_once(move |_, transaction| {
        *install_slot.lock() = Some(transaction);
        true
    });
    let take_slot = Arc::clone(&slot);
    waf.expect_take_transaction()
        .return_once(move |_| take_slot.lock().take());
    let mut response_headers = http::HeaderMap::new();
    response_headers.append("x-test", http::HeaderValue::from_static("allowed"));
    response_headers.append("x-test", http::HeaderValue::from_static("blocked"));
    session.expect_response_header_map().return_const(response_headers);
    session.expect_get_response_status().return_const(Some(200_u16));

    let mut request_log = PluginLog::new(NAME);
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head_with_headers(
        &mut session,
        &mut waf,
        "/json",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 1024;
    session.expect_ctx().return_const(ctx);
    session
        .expect_capture_request_body_with_consumer()
        .times(1)
        .return_once(|_, consumer| {
            consumer.consume(b"private-invalid-json");
            Ok(20)
        });
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(20));
    session.expect_read_request_body().times(0);
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    use crate::request_body::BodyReadError;

    let plugin = plugin(
        "SecRule REQUEST_BODY \"@contains blocked\" \"id:5,phase:2,pass\"\n",
        WafMode::On,
    );
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/large");
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 8;
    session.expect_ctx().return_const(ctx);
    session
        .expect_ensure_request_body()
        .return_once(|_| Err(BodyReadError::BufferLimitExceeded { limit: 8 }));
    session.expect_record_error().returning(|_| ());
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/submit");
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
    waf.expect_install_transaction().times(1).return_once(|_, transaction| {
        drop(transaction);
        true
    });
    let mut log = PluginLog::new(NAME);

    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/submit");
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
    waf.expect_install_transaction()
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
    waf.expect_take_transaction()
        .times(2)
        .returning(move |_| take_slot.lock().take());
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::new());
    session.expect_get_response_status().return_const(Some(201_u16));

    let mut request_log = PluginLog::new(NAME);
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
        "lexical": {"maxRawTokenBytes": 65536, "maxDecodedBytes": 1048576,
            "maxEvents": 65536, "maxDepth": 64, "maxRetainedBytes": 1048576, "maxWork": 16777216},
        "structure": {"maxRetainedBytes": 1048576, "maxKeyBytes": 65536, "maxMembers": 65536,
            "maxPathBytes": 16384, "maxDepth": 64, "maxWork": 16777216},
        "results": {"maxCandidates": 65536, "maxFacts": 65536, "maxRetainedBytes": 1048576,
            "maxWork": 16777216, "maxDeferredEntries": 4096, "maxDeferredBytes": 1048576}
    }))
    .unwrap()
}

#[tokio::test]
async fn full_json_default_and_explicit_limits_use_chunk_capture_without_materialization() {
    for explicit in [false, true] {
        let mut config = super::fixture::config("SecRule ARGS:json.a \"@streq hit\" \"id:1,phase:2,pass\"\n");
        config.request_body.inspection = WafRequestBodyInspection::Full;
        config.request_body.json = explicit.then(explicit_json_config);
        let plugin = WafPlugin::try_new(&config).unwrap();
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        expect_request_head_with_headers(
            &mut session,
            &mut waf,
            "/json",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        session.expect_ctx().return_const(EdgionHttpContext::new());
        let body = Bytes::from_static(br#"{"a":"gone","a":"hit"}"#);
        let len = body.len() as u64;
        let mut sequence = mockall::Sequence::new();
        session
            .expect_capture_request_body_with_consumer()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_, consumer| {
                for chunk in body.chunks(5) {
                    consumer.consume(chunk);
                }
                Ok(len)
            });
        session
            .expect_request_body_available_len()
            .with(mockall::predicate::eq(false))
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| Ok(len));
        session.expect_read_request_body().times(0);
        waf.expect_install_transaction().times(1).return_once(|_, transaction| {
            assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
            true
        });
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn body_independent_json_forced_raw_publishes_empty_without_capture() {
    let mut config = super::fixture::config(
        "SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=On\"\nSecAction \"id:2,phase:2,pass\"\n",
    );
    config.request_body.inspection = WafRequestBodyInspection::Full;
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head_with_headers(
        &mut session,
        &mut waf,
        "/json",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.body_declared = Some(true);
    session.expect_ctx().return_const(ctx);
    session.expect_capture_request_body_with_consumer().times(0);
    session.expect_ensure_request_body().times(0);
    session.expect_read_request_body().times(0);
    session.expect_request_body_available_len().times(0);
    waf.expect_install_transaction().times(1).return_once(|_, transaction| {
        assert_eq!(transaction.next_phase(), Some(Phase::ResponseHeaders));
        assert_eq!(transaction.request_body_error(), None);
        true
    });
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn full_capture_proof_and_all_caps_precede_materialization() {
    for case in 0..3 {
        let mut config = super::fixture::config("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n");
        config.request_body.inspection = WafRequestBodyInspection::Full;
        config.request_body.json = Some(explicit_json_config());
        let plugin = WafPlugin::try_new(&config).unwrap();
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        expect_request_head_with_headers(
            &mut session,
            &mut waf,
            "/json",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = if case == 2 { 8 } else { 32 << 20 };
        session.expect_ctx().return_const(ctx);
        let len = match case {
            0 | 1 => 0,
            _ => 9,
        };
        session
            .expect_capture_request_body_with_consumer()
            .times(1)
            .return_once(move |_, _| Ok(len));
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
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    limits.lexical.max_work = 0;
    config.request_body.json = Some(limits);
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head_with_headers(
        &mut session,
        &mut waf,
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
    waf.expect_install_transaction().times(1).return_once(|_, _| true);
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn shared_read_length_mismatch_fails_closed() {
    let plugin = plugin("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n", WafMode::On);
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head(&mut session, &mut waf, "/");
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
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
        let mut waf = MockWafSessionCapability::new();
        expect_request_head_with_headers(
            &mut session,
            &mut waf,
            "/",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        session.expect_ctx().return_const(EdgionHttpContext::new());
        let len = body.len() as u64;
        let inspected = body.clone();
        session
            .expect_capture_request_body_with_consumer()
            .times(1)
            .return_once(move |_, consumer| {
                consumer.consume(&inspected);
                Ok(len)
            });
        session
            .expect_request_body_available_len()
            .times(1)
            .return_once(move |_| Ok(len));
        session
            .expect_read_request_body()
            .times(1)
            .return_once(move |_| Ok(body));
        waf.expect_install_transaction().times(1).return_once(move |_, mut tx| {
            view.replace_spans(view.generation(), &[(0..len, Bytes::from_static(b"changed"))])
                .unwrap();
            tx.publish_response_head(ResponseHead::new(200, &[])).unwrap();
            assert_eq!(
                tx.execute_response_headers().unwrap().intervention().unwrap().status(),
                409
            );
            true
        });
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
        let mut waf = MockWafSessionCapability::new();
        expect_request_head(&mut session, &mut waf, "/");
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
            waf.expect_install_transaction().times(1).return_once(|_, _| true);
        } else {
            session.expect_read_request_body().times(0);
            session.expect_record_error().returning(|_| ());
        }
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
        let mut waf = MockWafSessionCapability::new();
        expect_request_head(&mut session, &mut waf, "/allowed");
        session.expect_ensure_request_body().times(0);
        session.expect_read_request_body().times(0);
        if !deny {
            waf.expect_install_transaction().times(1).return_once(|_, _| true);
        }
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
    let mut waf = MockWafSessionCapability::new();
    expect_request_head_with_headers(
        &mut session,
        &mut waf,
        "/",
        vec![(b"content-type".to_vec(), b"application/json".to_vec())],
    );
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_capture_request_body_with_consumer()
        .times(1)
        .return_once(|_, consumer| {
            consumer.consume(br#"{"a":"hit"}"#);
            Ok(11)
        });
    session
        .expect_request_body_available_len()
        .times(1)
        .return_once(|_| Ok(11));
    session.expect_read_request_body().times(0);
    waf.expect_install_transaction().times(0);
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
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
        let mut waf = MockWafSessionCapability::new();
        waf.expect_record_terminal_error().times(1).returning(|_, _, _, _| ());
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        assert_eq!(
            plugin.runtime_failure(&mut session, WafLogPhase::RequestBody, &error),
            PluginRunningResult::ErrResponse { status, body: None }
        );
    }
}

#[test]
fn h2_authority_host_projection_preserves_exact_borrowed_bytes_and_semantics() {
    for authority in ["MiXeD.example.com:8443", "[2001:db8::1]:443"] {
        let uri: http::Uri = format!("https://{authority}/echo").parse().unwrap();
        let mut map = http::HeaderMap::new();
        map.insert("x-test", http::HeaderValue::from_static("safe"));
        let views = request_header_views(&map, "HTTP/2.0", uri.authority());
        assert_eq!(views.len(), 2);
        let host = views.iter().find(|h| h.name() == b"host").unwrap();
        assert_eq!(host.value(), authority.as_bytes());
        assert_eq!(
            host.value().as_ptr(),
            uri.authority().unwrap().as_str().as_ptr(),
            "borrow the original authority storage"
        );
        assert!(
            !map.contains_key("host"),
            "projection never changes the live header map"
        );
        let config=super::fixture::config(&format!(
            "SecRule &REQUEST_HEADERS:Host \"@eq 1\" \"id:1,phase:1,pass,log,t:none\"\nSecRule REQUEST_HEADERS_NAMES \"@streq host\" \"id:2,phase:1,pass,log,t:none\"\nSecRule REQUEST_HEADERS:Host \"@streq {authority}\" \"id:3,phase:1,pass,log,t:none\"\n"));
        let generation = config.compile_generation().unwrap();
        let head = RequestHead::new(
            b"GET /echo HTTP/2.0",
            b"GET",
            b"/echo",
            b"/echo",
            b"HTTP/2.0",
            b"192.0.2.1",
            b"authority-test",
            &views,
        );
        let mut tx = Transaction::new(&generation, head, RuntimeLimits::default()).unwrap();
        assert_eq!(
            tx.execute_request_headers()
                .unwrap()
                .findings()
                .iter()
                .map(|f| f.rule_id())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }
}

#[test]
fn host_projection_never_replaces_explicit_empty_invalid_or_duplicate_hosts() {
    let uri: http::Uri = "https://authority.example.com:8443/".parse().unwrap();
    for values in [
        vec![b"".as_slice()],
        vec![b"\xff".as_slice()],
        vec![b"first.example.com".as_slice(), b"second.example.com".as_slice()],
    ] {
        let mut map = http::HeaderMap::new();
        for value in &values {
            map.append(http::header::HOST, http::HeaderValue::from_bytes(value).unwrap());
        }
        let views = request_header_views(&map, "HTTP/2.0", uri.authority());
        assert_eq!(views.iter().map(|h| h.value()).collect::<Vec<_>>(), values);
        assert_eq!(views.len(), map.len());
    }
}

#[test]
fn h1_and_missing_h2_authority_do_not_manufacture_host() {
    let map = http::HeaderMap::new();
    let absolute: http::Uri = "http://authority.example.com:8080/echo".parse().unwrap();
    let relative: http::Uri = "/echo".parse().unwrap();
    for (protocol, uri) in [
        ("HTTP/1.1", Some(&absolute)),
        ("HTTP/1.0", Some(&absolute)),
        ("HTTP/2.0", Some(&relative)),
        ("HTTP/2.0", None),
    ] {
        assert!(request_header_views(&map, protocol, uri.and_then(http::Uri::authority)).is_empty());
    }
    let mut empty = http::HeaderMap::new();
    empty.insert(http::header::HOST, http::HeaderValue::from_static(""));
    assert_eq!(
        request_header_views(&empty, "HTTP/1.1", absolute.authority())[0].value(),
        b""
    );
}

#[tokio::test]
async fn waf_expensive_checks_use_the_request_gateway_snapshot() {
    let rules = concat!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n",
        include_str!("../../../../../../edgion-waf/tests/fixtures/expensive-checks-crs-4.25.1.conf")
    );
    let plugin = plugin(rules, WafMode::On);
    for enabled in [false, true] {
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        let runtime = crate::config::ResolvedGatewayConfig {
            waf_expensive_checks: enabled,
            ..Default::default()
        };
        expect_request_head_with_context(
            &mut session,
            &mut waf,
            "/?a=one&a=two",
            vec![],
            EdgionHttpContext::with_gateway_config(Arc::new(runtime)),
        );
        session.expect_ctx().return_const(EdgionHttpContext::default());
        session.expect_ensure_request_body().return_once(|_| Ok(0));
        session.expect_request_body_available_len().return_once(|_| Ok(0));
        session.expect_read_request_body().return_once(|_| Ok(Bytes::new()));
        waf.expect_install_transaction().return_once(move |_, tx| {
            assert_eq!(
                tx.tx_value(b"paramcounter_ARGS_NAMES:a"),
                enabled.then_some(b"2".as_slice())
            );
            assert_eq!(
                tx.tx_value(b"inbound_anomaly_score_pl3"),
                enabled.then_some(b"5".as_slice())
            );
            true
        });
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn format_hard_limit_uses_selected_processor_without_body_rule_or_capture() {
    for inspection in [WafRequestBodyInspection::Prefix, WafRequestBodyInspection::Full] {
        for (content_type, expected) in [
            ("application/json", Some(2048)),
            ("application/xml", Some(1024)),
            ("text/plain", None),
        ] {
            let mut config = super::fixture::config("SecAction \"id:1,phase:1,pass\"\n");
            config.mode = WafMode::DetectionOnly;
            config.request_body.inspection = inspection;
            config.request_body.max_json_body_size = Some("2KiB".into());
            config.request_body.max_xml_body_size = Some("1KiB".into());
            let plugin = WafPlugin::try_new(&config).unwrap();
            let mut session = MockPluginSession::new();
            let mut waf = MockWafSessionCapability::new();
            expect_request_head_with_headers(
                &mut session,
                &mut waf,
                "/",
                vec![(b"content-type".to_vec(), content_type.as_bytes().to_vec())],
            );
            session.expect_ctx().return_const(EdgionHttpContext::default());
            waf.expect_install_body_limit()
                .times(usize::from(expected.is_some()))
                .returning(move |_, limit| {
                    assert_eq!(Some(limit), expected);
                    Ok(())
                });
            session.expect_capture_request_body_with_consumer().times(0);
            session.expect_read_request_body_prefix().times(0);
            waf.expect_install_transaction().times(1).returning(|_, _| true);
            let mut log = PluginLog::new("Waf");
            session
                .expect_waf()
                .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
            assert!(matches!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::GoodNext
            ));
        }
    }
}

#[tokio::test]
async fn format_hard_limit_respects_phase_one_processor_and_cannot_be_detection_only_bypassed() {
    let mut config = super::fixture::config("SecAction \"id:1,phase:1,pass,ctl:requestBodyProcessor=JSON\"\n");
    config.mode = WafMode::DetectionOnly;
    config.request_body.inspection = WafRequestBodyInspection::Full;
    config.request_body.max_json_body_size = Some("8".into());
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    expect_request_head_with_headers(
        &mut session,
        &mut waf,
        "/",
        vec![(b"content-type".to_vec(), b"text/plain".to_vec())],
    );
    waf.expect_install_body_limit().times(1).returning(|_, limit| {
        assert_eq!(limit, 8);
        Err(BodyReadError::BufferLimitExceeded { limit })
    });
    waf.expect_install_transaction().times(0);
    let mut log = PluginLog::new("Waf");
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
    assert!(matches!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    ));
}

#[test]
fn explicit_policy_stop_continue_and_reject_apply_in_detection_only() {
    for action in [WafInspectionLimitAction::Continue, WafInspectionLimitAction::Reject] {
        let mut config = super::fixture::config("SecAction \"id:1,phase:1,pass\"\n");
        config.mode = WafMode::DetectionOnly;
        config.request_body.inspection = WafRequestBodyInspection::Full;
        config.request_body.on_limit = Some(action);
        let plugin = WafPlugin::try_new(&config).unwrap();
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        let head = RequestHead::new(
            b"GET / HTTP/1.1",
            b"GET",
            b"/",
            b"/",
            b"HTTP/1.1",
            b"192.0.2.1",
            b"id",
            &[],
        );
        let mut transaction = Transaction::new_shared_with_policy(
            Arc::clone(&plugin.shared.generation),
            head,
            RuntimeLimits::default(),
            edgion_waf::seclang::EvaluationMode::Accelerated,
            plugin.shared.engine_mode,
            RuleCheckPolicy::default(),
        )
        .unwrap();
        transaction.execute_request_headers().unwrap();
        waf.expect_record_body_cutoff().times(1).returning(|_, bytes, storage| {
            assert_eq!(bytes, 32);
            assert!(storage);
            true
        });
        if action == WafInspectionLimitAction::Reject {
            waf.expect_record_terminal_error()
                .times(1)
                .returning(|_, _, code, status| {
                    assert!(matches!(code, WafErrorCode::BodyLimit));
                    assert_eq!(status, 413);
                });
        } else {
            waf.expect_record_phase_outcome().times(1).returning(|_, phase, _, _| {
                assert_eq!(phase, WafLogPhase::RequestBody);
            });
            waf.expect_install_transaction().times(1).returning(|_, tx| {
                assert_eq!(tx.next_phase(), Some(Phase::ResponseHeaders));
                true
            });
        }
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        let result = plugin.inspection_stop(
            &mut session,
            json_capture::InspectionPolicy {
                cap: 128,
                window: 32,
                native_prefix: true,
                storage_limit: true,
            },
            transaction,
        );
        assert_eq!(
            result,
            if action == WafInspectionLimitAction::Reject {
                PluginRunningResult::ErrResponse {
                    status: 413,
                    body: None,
                }
            } else {
                PluginRunningResult::GoodNext
            }
        );
    }
}

#[tokio::test]
async fn request_orchestration_uses_storage_window_only_for_fresh_waf_only_capture() {
    for case in 0..4 {
        let mut config = super::fixture::config(concat!(
            "SecRule REQUEST_BODY \"@contains blocked\" \"id:1,phase:2,deny\"\n",
            "SecRule REQUEST_URI \"@streq /restricted\" \"id:2,phase:2,deny,status:409\"\n",
        ));
        config.mode = WafMode::On;
        config.request_body.inspection = WafRequestBodyInspection::Full;
        config.request_body.max_inspect_size = (case != 3).then(|| "64".to_owned());
        let plugin = WafPlugin::try_new(&config).unwrap();
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        expect_request_head(&mut session, &mut waf, if case == 2 { "/restricted" } else { "/" });
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = 128;
        ctx.request_body_config.max_buffered_size_bytes = 32;
        ctx.request_body_config.memory_buffer_bytes = 16;
        set_partial_capture_policy(&mut ctx, &plugin, case != 1);
        if case == 2 {
            let (mut storage, view) = crate::request_body::BodyStorage::new(128, 128);
            storage.write(&Bytes::from(vec![b'x'; 65])).await.unwrap();
            storage.finish().await.unwrap();
            ctx.request_body.install(view);
        }
        session.expect_ctx().return_const(ctx);
        if case == 1 {
            session.expect_ensure_request_body().times(1).return_once(|cap| {
                assert_eq!(cap, Some(128));
                Err(BodyReadError::BufferLimitExceeded { limit: 32 })
            });
            session.expect_record_error().returning(|_| ());
        } else {
            if case == 2 {
                session.expect_ensure_request_body().times(1).return_once(|_| Ok(65));
                session
                    .expect_request_body_available_len()
                    .times(1)
                    .return_once(|_| Ok(65));
            } else {
                session
                    .expect_capture_request_body_inspection()
                    .times(1)
                    .return_once(|window, consumer| {
                        assert_eq!(window, 32);
                        assert_eq!(consumer.source_window(), Some(32));
                        Ok(crate::plugins::runtime::traits::session::RequestBodyInspectionCapture {
                            captured_len: 33,
                            complete: false,
                            storage: None,
                        })
                    });
            }
            waf.expect_record_body_cutoff()
                .times(1)
                .returning(move |_, bytes, storage| {
                    assert_eq!(bytes, if case == 2 { 64 } else { 32 });
                    assert_eq!(storage, case != 2);
                    true
                });
            waf.expect_install_transaction()
                .times(usize::from(case != 2))
                .returning(|_, tx| {
                    assert_eq!(tx.next_phase(), Some(Phase::ResponseHeaders));
                    true
                });
        }
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            match case {
                1 => PluginRunningResult::ErrResponse {
                    status: 413,
                    body: None,
                },
                2 => PluginRunningResult::ErrResponse {
                    status: 409,
                    body: None,
                },
                _ => PluginRunningResult::GoodNext,
            }
        );
    }
}

#[tokio::test]
async fn default_full_json_and_xml_sources_can_exceed_the_former_sixteen_mib_cap() {
    for xml in [false, true] {
        let plugin = plugin(
            if xml {
                r#"SecRule XML:/* "@contains blocked" "id:1,phase:2,deny""#
            } else {
                r#"SecRule ARGS "@contains blocked" "id:1,phase:2,deny""#
            },
            WafMode::On,
        );
        let mut source = if xml {
            format!("<!--{}-->", " ".repeat(64 * 1024)).repeat(257).into_bytes()
        } else {
            vec![b' '; (16 << 20) + 1]
        };
        source.extend_from_slice(if xml {
            b"<r>safe</r>".as_slice()
        } else {
            br#"{"a":"safe"}"#.as_slice()
        });
        let len = source.len() as u64;
        let mut session = MockPluginSession::new();
        let mut waf = MockWafSessionCapability::new();
        expect_request_head_with_headers(
            &mut session,
            &mut waf,
            "/large",
            vec![(
                b"content-type".to_vec(),
                if xml {
                    b"application/xml".to_vec()
                } else {
                    b"application/json".to_vec()
                },
            )],
        );
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_capture_request_body_with_consumer()
            .times(1)
            .return_once(move |cap, consumer| {
                assert_eq!(cap, Some(32 << 20));
                for part in source.chunks(64 * 1024) {
                    consumer.consume(part);
                }
                Ok(len)
            });
        session
            .expect_request_body_available_len()
            .times(1)
            .return_once(move |_| Ok(len));
        session.expect_read_request_body().times(0);
        waf.expect_install_transaction().times(1).return_once(|_, _| true);
        session
            .expect_waf()
            .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
        assert_eq!(
            plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn incompatible_inspection_window_rejects_before_waf_body_reads_even_in_detection_only() {
    let mut config = super::fixture::config("SecRule REQUEST_BODY \"@contains blocked\" \"id:1,phase:2,deny\"\n");
    config.mode = WafMode::DetectionOnly;
    config.request_body.max_inspect_size = Some("129".to_owned());
    let plugin = WafPlugin::try_new(&config).unwrap();
    let mut session = MockPluginSession::new();
    let mut waf = MockWafSessionCapability::new();
    waf.expect_record_terminal_error()
        .times(1)
        .returning(|_, phase, code, status| {
            assert_eq!(phase, WafLogPhase::RequestBody);
            assert!(matches!(code, WafErrorCode::RuntimeInvariant));
            assert_eq!(status, 500);
        });
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 128;
    set_partial_capture_policy(&mut ctx, &plugin, true);
    let mut initial_ctx = EdgionHttpContext::new();
    initial_ctx.request_body_config = ctx.request_body_config;
    set_partial_capture_policy(&mut initial_ctx, &plugin, ctx.waf_partial_capture_allowed());
    expect_request_head_with_context(&mut session, &mut waf, "/", Vec::new(), initial_ctx);
    session.expect_ctx().return_const(ctx);
    session.expect_ensure_request_body().times(0);
    session.expect_capture_request_body_inspection().times(0);
    session.expect_capture_request_body_with_consumer().times(0);
    session.expect_read_request_body().times(0);
    waf.expect_record_body_cutoff().times(0);
    waf.expect_install_transaction().times(0);
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new(NAME)).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}
