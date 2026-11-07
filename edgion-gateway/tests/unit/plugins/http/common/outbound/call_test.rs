use super::*;

#[test]
fn auth_metadata_wins_over_forwarded_origin_and_custom_headers() {
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::common::outbound_request::{
        CustomField, CustomSource, ListFields, OriginField, PresencePolicy,
    };

    let metadata = [
        ("X-Forwarded-Host", "rewritten.test"),
        ("X-Forwarded-Uri", "/graphql"),
        ("X-Forwarded-Method", "POST"),
        ("X-Forwarded-Query", "mode=rest"),
    ];
    for forward_all in [false, true] {
        for configured in [false, true] {
            let mut spec = OutboundRequestSpec::default();
            spec.headers = ListFields {
                forward_all,
                origin: if configured {
                    metadata
                        .iter()
                        .map(|(name, _)| OriginField {
                            name: (*name).into(),
                            presence: PresencePolicy::Required,
                        })
                        .chain(std::iter::once(OriginField {
                            name: "X-Selected".into(),
                            presence: PresencePolicy::Required,
                        }))
                        .collect()
                } else {
                    vec![]
                },
                custom: if configured {
                    metadata
                        .iter()
                        .map(|(name, _)| CustomField {
                            name: name.to_ascii_lowercase(),
                            source: CustomSource::Value("custom-spoof".into()),
                        })
                        .collect()
                } else {
                    vec![]
                },
            };
            let mut session = MockPluginSession::new();
            setup_header_mocks(
                &mut session,
                &[
                    ("X-Forwarded-Host", "inbound-spoof"),
                    ("X-Forwarded-Host", "second-spoof"),
                    ("X-Forwarded-Uri", "/spoof"),
                    ("X-Forwarded-Method", "DELETE"),
                    ("X-Forwarded-Query", "spoof=1"),
                    ("X-Unselected", "omit-me"),
                    ("X-Selected", "selected"),
                ],
            );
            session.expect_get_request_method().return_const(Some("GET"));
            session.expect_get_request_path().return_const("/users/42".to_string());
            let mut req = CompiledRequest::from_spec(&spec)
                .unwrap()
                .build_request(&session, None, "http://auth.test", None, &mut Default::default())
                .unwrap();
            if configured {
                assert_eq!(req.headers()["x-forwarded-method"], "custom-spoof");
            }
            finalize_request(&mut req, &metadata, None).unwrap();
            for (name, value) in metadata {
                assert_eq!(req.headers().get(name).unwrap(), value);
                assert_eq!(req.headers().get_all(name).iter().count(), 1);
            }
            assert_eq!(req.headers().contains_key("x-unselected"), forward_all);
            assert_eq!(req.headers().contains_key("x-selected"), forward_all || configured);
            // Auth-hop defaults describe entry identity, independently of metadata.
            assert_eq!(req.method(), reqwest::Method::GET);
            assert_eq!(req.url().path(), "/users/42");
            assert!(req.body().is_none());
        }
    }
}

#[test]
fn forwarded_current_body_owns_final_bytes_and_framing() {
    for body in [Bytes::from_static(b"rewritten body"), Bytes::new()] {
        let mut req = reqwest::Request::new(reqwest::Method::POST, "http://auth.test/verify".parse().unwrap());
        req.headers_mut().insert("content-length", "99".parse().unwrap());
        req.headers_mut()
            .insert("transfer-encoding", "chunked".parse().unwrap());
        finalize_request(&mut req, &[], Some(body.clone())).unwrap();
        assert_eq!(req.body().unwrap().as_bytes().unwrap(), body.as_ref());
        assert_eq!(req.headers()["content-length"], body.len().to_string());
        assert!(!req.headers().contains_key("transfer-encoding"));
    }
}

#[test]
fn outbound_response_debug_omits_auth_payload() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-private-token", "private-token".parse().unwrap());
    let response = OutboundResponse {
        status: 200,
        headers,
        body: Bytes::from_static(b"private-body"),
    };
    for debug in [format!("{response:?}"), format!("{response:#?}")] {
        assert!(debug.contains("200"));
        assert!(!debug.contains("private"));
    }
    assert_eq!(response.body.as_ref(), b"private-body");
    assert_eq!(response.headers["x-private-token"], "private-token");
}

#[test]
fn outbound_call_error_debug_and_display_are_static_and_detail_free() {
    const CREDENTIAL_MARKER: &str = "credential-marker-must-not-escape";
    let cases = [
        (
            OutboundCallError::Unhealthy,
            "Unhealthy",
            "outbound target is unhealthy",
        ),
        (
            OutboundCallError::RateLimited,
            "RateLimited",
            "outbound rate limit exceeded",
        ),
        (OutboundCallError::Timeout, "Timeout", "outbound request timed out"),
        (OutboundCallError::Connect, "Connect", "outbound connection error"),
        (
            OutboundCallError::ResponseTooLarge,
            "ResponseTooLarge",
            "outbound response body exceeded size cap",
        ),
        (OutboundCallError::Exhausted, "Exhausted", "outbound retries exhausted"),
        (
            OutboundCallError::AuthorityRejected,
            "AuthorityRejected",
            "outbound request rejected: invalid upstream authority",
        ),
    ];

    for (error, expected_debug, expected_display) in cases {
        let debug = format!("{error:?}");
        let display = error.to_string();
        assert_eq!(debug, expected_debug);
        assert_eq!(display, expected_display);
        assert!(!debug.contains(CREDENTIAL_MARKER));
        assert!(!display.contains(CREDENTIAL_MARKER));
    }
}
use std::time::Duration;

use crate::plugins::http::common::http_client::get_http_client_arc;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::http::common::outbound::health::{HealthTarget, OutboundHealth};
use crate::plugins::http::common::outbound::request::CompiledRequest;
use crate::plugins::http::common::outbound::target::ResolvedTarget;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
use edgion_resources::resources::common::request_template::CompiledTemplate;

// ── Helpers ──────────────────────────────────────────────────────────────

/// Compile a template string for test use, panicking on a compile error.
fn compile_tpl(s: &str) -> CompiledTemplate {
    CompiledTemplate::compile(s).unwrap()
}

/// Build a minimal [`ManagedOutboundClient`] with a static target.
fn make_client(url: &str) -> ManagedOutboundClient {
    ManagedOutboundClient {
        http_client: get_http_client_arc(),
        resolved_target: ResolvedTarget::Static(url.to_string()),
        compiled_request: CompiledRequest::from_spec(&OutboundRequestSpec::default())
            .expect("default spec must compile"),
        health: OutboundHealth::new(HealthTarget::Static(url.to_string()), None),
        rate: None,
        success: None,
        max_response_bytes: 64 * 1024,
        timeout: Duration::from_secs(5),
        timeout_template: None,
        retry: None,
        egress_policy: None,
        health_task: None,
        resolved_secrets: None,
    }
}

fn non_cloneable_request() -> reqwest::Request {
    let mut request = reqwest::Request::new(
        reqwest::Method::POST,
        reqwest::Url::parse("http://example.com/").unwrap(),
    );
    let body = futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"streaming body")) });
    *request.body_mut() = Some(reqwest::Body::wrap_stream(body));
    assert!(request.try_clone().is_none());
    request
}

#[test]
fn sole_attempt_takes_non_cloneable_request() {
    let mut template = Some(non_cloneable_request());

    assert!(request_for_attempt(&mut template, 1).is_some());
    assert!(template.is_none());
}

#[test]
fn retry_attempt_rejects_non_cloneable_request_without_consuming_template() {
    let mut template = Some(non_cloneable_request());

    assert!(request_for_attempt(&mut template, 2).is_none());
    assert!(template.is_some());
}

#[test]
fn retry_attempt_preserves_request_fields_through_final_transfer() {
    let mut request = reqwest::Request::new(
        reqwest::Method::POST,
        reqwest::Url::parse("http://example.com/replay?mode=full").unwrap(),
    );
    *request.body_mut() = Some(Bytes::from_static(b"replayable body").into());
    *request.timeout_mut() = Some(Duration::from_millis(750));
    *request.version_mut() = http::Version::HTTP_2;
    let mut credential = http::HeaderValue::from_static("test-credential");
    credential.set_sensitive(true);
    request.headers_mut().insert(http::header::AUTHORIZATION, credential);
    let mut template = Some(request);

    let first = request_for_attempt(&mut template, 2).unwrap();
    assert!(template.is_some());
    let final_request = request_for_attempt(&mut template, 1).unwrap();
    assert!(template.is_none());
    for request in [first, final_request] {
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), "http://example.com/replay?mode=full");
        assert_eq!(request.timeout(), Some(&Duration::from_millis(750)));
        assert_eq!(request.version(), http::Version::HTTP_2);
        assert_eq!(request.headers()[http::header::AUTHORIZATION], "test-credential");
        assert!(request.headers()[http::header::AUTHORIZATION].is_sensitive());
        assert_eq!(request.body().unwrap().as_bytes(), Some(b"replayable body".as_slice()));
    }
}

// ── TDD tests ─────────────────────────────────────────────────────────────

/// `resolve_timeout` clamps an over-ceiling render to `MAX_OUTBOUND_TIMEOUT_MS`,
/// tested directly since driving a 60s+ real timeout end-to-end is impractical.
#[test]
fn resolve_timeout_clamps_to_ceiling() {
    use edgion_resources::resources::common::external_http::MAX_OUTBOUND_TIMEOUT_MS;

    let mut client = make_client("http://10.0.0.1:1");
    client.timeout = Duration::from_secs(5);
    client.timeout_template = Some(compile_tpl("${header:X-Test-Timeout}"));

    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-Test-Timeout").unwrap(),
            http::HeaderValue::from_static("999999999"),
        )]));

    let resolved = resolve_timeout(&client, TemplateContext::Request(&session));
    assert_eq!(resolved, Duration::from_millis(MAX_OUTBOUND_TIMEOUT_MS));
}

/// Sanity: no `timeout_template` set means `resolve_timeout` returns the static
/// `timeout` unchanged.
#[test]
fn resolve_timeout_uses_static_when_no_template() {
    let mut client = make_client("http://10.0.0.1:1");
    client.timeout = Duration::from_secs(7);
    client.timeout_template = None;

    let session = MockPluginSession::new();
    assert_eq!(
        resolve_timeout(&client, TemplateContext::Request(&session)),
        Duration::from_secs(7)
    );
}

/// CRITICAL 2 regression: a render below `MIN_OUTBOUND_TIMEOUT_MS` (e.g. "1")
/// must fall back to the static timeout, not be honored as-is. Without a
/// floor, a tiny-but-nonzero render would pin the deadline low enough that
/// the outbound call always times out — a reliable DoS.
#[test]
fn resolve_timeout_floor_falls_back_to_static_timeout() {
    let mut client = make_client("http://10.0.0.1:1");
    client.timeout = Duration::from_secs(5);
    client.timeout_template = Some(compile_tpl("${ctx:desired_timeout_ms}"));

    let mut session = MockPluginSession::new();
    session
        .expect_get_ctx_var()
        .withf(|n| n == "desired_timeout_ms")
        .return_const(Some("1".to_string()));

    let resolved = resolve_timeout(&client, TemplateContext::Request(&session));
    assert_eq!(resolved, Duration::from_secs(5), "must fall back to the static timeout");
}

/// A render exactly at `MIN_OUTBOUND_TIMEOUT_MS` is accepted (the floor is
/// inclusive), not fallen back.
#[test]
fn resolve_timeout_accepts_render_at_floor() {
    use edgion_resources::resources::common::external_http::MIN_OUTBOUND_TIMEOUT_MS;

    let mut client = make_client("http://10.0.0.1:1");
    client.timeout = Duration::from_secs(5);
    client.timeout_template = Some(compile_tpl("${ctx:desired_timeout_ms}"));

    let mut session = MockPluginSession::new();
    session
        .expect_get_ctx_var()
        .withf(|n| n == "desired_timeout_ms")
        .return_const(Some(MIN_OUTBOUND_TIMEOUT_MS.to_string()));

    let resolved = resolve_timeout(&client, TemplateContext::Request(&session));
    assert_eq!(resolved, Duration::from_millis(MIN_OUTBOUND_TIMEOUT_MS));
}

// ── Backoff tests (Task 1.2) ─────────────────────────────────────────────

#[test]
fn backoff_exponential_no_jitter_is_exact() {
    use edgion_resources::resources::common::external_http::{BackoffPolicy, WebhookRetry};
    let retry = WebhookRetry {
        max_retries: 3,
        retry_delay_ms: 100,
        retry_on_timeout: None,
        retry_on_connect_error: true,
        retry_on_status: vec![],
        backoff_policy: BackoffPolicy::Exponential,
        max_delay_ms: 10_000,
        jitter: false,
        honor_retry_after: false,
        retry_on_body_failure: false,
    };
    assert_eq!(compute_retry_delay_ms(1, &retry), 100);
    assert_eq!(compute_retry_delay_ms(2, &retry), 200);
    assert_eq!(compute_retry_delay_ms(3, &retry), 400);
}

#[test]
fn backoff_fixed_is_constant() {
    use edgion_resources::resources::common::external_http::BackoffPolicy;
    let mut retry = crate::plugins::http::common::outbound::call::tests_retry(100);
    retry.backoff_policy = BackoffPolicy::Fixed;
    retry.jitter = false;
    assert_eq!(compute_retry_delay_ms(1, &retry), 100);
    assert_eq!(compute_retry_delay_ms(3, &retry), 100);
}

#[test]
fn backoff_jitter_stays_in_band_and_under_cap() {
    let mut retry = crate::plugins::http::common::outbound::call::tests_retry(1000);
    retry.max_delay_ms = 1000;
    retry.jitter = true;
    let mut all_equal = true;
    let first = compute_retry_delay_ms(5, &retry); // exp would be huge → capped at 1000
    for _ in 0..50 {
        let d = compute_retry_delay_ms(5, &retry);
        assert!((500..=1000).contains(&d), "jittered {d} out of [500,1000]");
        if d != first {
            all_equal = false;
        }
    }
    assert!(!all_equal, "jitter must vary");
}

#[tokio::test]
async fn reqwest_header_transfer_preserves_body_stream() {
    let mut source = http::Response::new(reqwest::Body::from("payload"));
    let mut value = reqwest::header::HeaderValue::from_str(&"private-value".repeat(8)).unwrap();
    value.set_sensitive(true);
    let original_ptr = value.as_bytes().as_ptr();
    source.headers_mut().insert("x-private", value);
    let mut response: reqwest::Response = source.into();
    let (headers, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        std::mem::take(OutboundHttpResponse::headers_mut(&mut response))
    });
    assert_eq!(stats.calls, 0);
    assert_eq!(headers["x-private"].as_bytes().as_ptr(), original_ptr);
    assert!(headers["x-private"].is_sensitive());
    assert!(response.headers().is_empty());
    let (body, truncated) = stream_body_chunks_capped(OutboundHttpResponse::bytes_stream(response), 64)
        .await
        .unwrap();
    assert_eq!(body, Bytes::from_static(b"payload"));
    assert!(!truncated);
}
