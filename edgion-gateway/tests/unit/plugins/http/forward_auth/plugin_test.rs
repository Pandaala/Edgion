use super::*;

use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use edgion_resources::resources::common::success::{BodyPredicate, PredicateOp, SuccessSpec};
use edgion_resources::resources::edgion_plugins::ForwardAuthDecision;
use mockall::Sequence;
use reqwest::header::{HeaderMap, HeaderValue};

// ── Helpers ──────────────────────────────────────────────────────────────

fn make_forward_auth(decision: ForwardAuthDecision) -> ForwardAuth {
    let client = ManagedOutboundClient::test_with_static_target("http://example.com");
    let mut config = ForwardAuthConfig {
        decision,
        ..ForwardAuthConfig::default()
    };
    config.conn.status_on_error = 503;
    ForwardAuth::from_client(&config, client)
}

fn make_forward_auth_with_success(decision: ForwardAuthDecision, success: SuccessSpec) -> ForwardAuth {
    let mut client = ManagedOutboundClient::test_with_static_target("http://example.com");
    client.success = Some(success);
    let mut config = ForwardAuthConfig {
        decision,
        ..ForwardAuthConfig::default()
    };
    config.conn.status_on_error = 503;
    ForwardAuth::from_client(&config, client)
}

fn make_request_session() -> MockPluginSession {
    let mut s = MockPluginSession::new();
    setup_header_mocks(&mut s, &[("host", "example.com")]);
    s.expect_get_path().return_const("/api/data".to_string());
    s.expect_get_method().return_const("GET".to_string());
    s.expect_request_query().return_const(None);
    s
}

fn response(status: u16, headers: HeaderMap, body: Bytes) -> OutboundResponse {
    OutboundResponse { status, headers, body }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[test]
fn context_metadata_describes_current_request_after_rewrite_or_without_rewrite() {
    for (host, path, method, query) in [
        ("entry.test", "/users/42", "GET", Some("verbose=1")),
        ("rewritten.test", "/graphql", "POST", Some("mode=rest")),
        ("entry.test", "/users/42", "GET", None),
        ("entry.test", "/users/42", "GET", Some("")),
        (
            "rewritten.test",
            "/graphql",
            "POST",
            Some("key=one&key=two&encoded=a%20b%26c"),
        ),
    ] {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("host", host), ("X-Forwarded-Method", "spoofed")]);
        session.expect_get_path().return_const(path.to_string());
        session.expect_get_method().return_const(method.to_string());
        session.expect_request_query().return_const(query);
        // Entry accessors are deliberately unset: metadata must use the live view.
        let headers = ForwardAuth::context_headers(&session);
        assert_eq!(
            headers,
            [
                ("X-Forwarded-Host", host),
                ("X-Forwarded-Uri", path),
                ("X-Forwarded-Method", method),
                ("X-Forwarded-Query", query.unwrap_or_default()),
            ]
        );
        if let Some(query) = session.request_query().filter(|query| !query.is_empty()) {
            assert!(std::ptr::eq(headers[3].1, query));
        }
    }
}

#[tokio::test]
async fn forward_body_reads_current_committed_body_before_outbound_gate() {
    let mut fa = make_forward_auth(ForwardAuthDecision::default());
    fa.forward_body = true;
    fa.client.health.force_unhealthy_for_test();
    let mut session = make_request_session();
    session
        .expect_read_request_body()
        .withf(|limit| limit.is_none())
        .times(1)
        .returning(|_| Ok(Bytes::from_static(b"rewritten body")));
    session.expect_record_error().return_const(());
    session.expect_record_outcome().return_const(());
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().return_const(());
    let result = fa.run_request(&mut session, &mut PluginLog::new("ForwardAuth")).await;
    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
}

#[test]
fn name_is_static_across_instances() {
    let first = make_forward_auth(ForwardAuthDecision::default());
    let second = make_forward_auth(ForwardAuthDecision::default());

    assert_eq!(first.name(), "ForwardAuth");
    assert_eq!(second.name(), "ForwardAuth");
    assert!(std::ptr::eq(first.name(), second.name()));
}

#[tokio::test]
async fn pass_copies_upstream_headers() {
    let decision = ForwardAuthDecision {
        upstream_headers: vec!["x-user-id".to_string()],
        hide_credentials: true,
        ..ForwardAuthDecision::default()
    };
    let fa = make_forward_auth(decision);

    let mut headers = HeaderMap::new();
    headers.append("x-user-id", HeaderValue::from_static("user123"));
    headers.append("x-user-id", HeaderValue::from_static("ignored-second-value"));
    let outbound = response(200, headers, Bytes::from_static(b"ok"));

    let mut session = MockPluginSession::new();
    let mut seq = Sequence::new();
    session
        .expect_set_request_header()
        .withf(|name, value| name == "x-user-id" && value == "user123")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "authorization")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| Ok(()));

    let mut log = PluginLog::new("ForwardAuth");
    let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

    assert!(matches!(result, PluginRunningResult::GoodNext));
    assert_eq!(log.render_for_test(), "auth pass 200");
}

#[tokio::test]
async fn denial_does_not_copy_framing_or_any_connection_nominated_field() {
    for opaque_connection in [false, true] {
        let fa = make_forward_auth(ForwardAuthDecision {
            client_headers: vec![
                "Content-Length".into(),
                "Proxy-Connection".into(),
                "X-First".into(),
                "X-Second".into(),
                "Content-Type".into(),
                "WWW-Authenticate".into(),
                "content-LENGTH".into(),
                "x-FIRST".into(),
            ],
            ..ForwardAuthDecision::default()
        });
        let mut headers = HeaderMap::new();
        headers.append("connection", HeaderValue::from_static(" , X-First, x-FIRST, , \t"));
        headers.append(
            "connection",
            HeaderValue::from_static("x-SECOND, Content-Type, X-FIRST, x-second"),
        );
        if opaque_connection {
            headers.append("connection", HeaderValue::from_bytes(b"\xff").unwrap());
        }
        for (name, value) in [
            ("content-length", "999"),
            ("proxy-connection", "keep-alive"),
            ("x-first", "private"),
            ("x-second", "private"),
            ("content-type", "text/plain"),
            ("www-authenticate", "Bearer"),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
        }
        let mut session = MockPluginSession::new();
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |response, _| {
                for name in [
                    "content-length",
                    "proxy-connection",
                    "x-first",
                    "x-second",
                    "content-type",
                ] {
                    assert!(!response.headers.contains_key(name), "{name}");
                }
                assert_eq!(response.headers.contains_key("www-authenticate"), !opaque_connection);
                // The local response owner still controls its own connection.
                assert_eq!(response.headers["connection"], "close");
                Ok(())
            });
        session.expect_write_response_body().times(1).returning(|body, end| {
            assert_eq!(body.unwrap(), Bytes::from_static(b"denied"));
            assert!(end);
            Ok(())
        });
        session.expect_shutdown().times(1).returning(|| {});
        let mut log = PluginLog::new("ForwardAuth");
        assert_eq!(
            fa.handle_outbound_response(
                &mut session,
                &mut log,
                response(401, headers, Bytes::from_static(b"denied")),
            )
            .await,
            PluginRunningResult::ErrTerminateRequest,
        );
        assert_eq!(
            log.render_for_test().contains("invalid auth Connection header"),
            opaque_connection
        );
    }
}

#[tokio::test]
async fn deny_copies_client_headers_and_shortcircuits() {
    let decision = ForwardAuthDecision {
        client_headers: vec![
            "x-auth-reason".to_string(),
            "WWW-Authenticate".to_string(),
            "Set-Cookie".to_string(),
            "www-authenticate".to_string(),
            "set-cookie".to_string(),
        ],
        ..ForwardAuthDecision::default()
    };
    let fa = make_forward_auth(decision);

    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("text/plain; charset=utf-8"));
    headers.insert("x-auth-reason", HeaderValue::from_static("expired"));
    headers.append("www-authenticate", HeaderValue::from_static("Bearer realm=\"primary\""));
    headers.append("www-authenticate", HeaderValue::from_bytes(b"\xff").unwrap());
    headers.append("www-authenticate", HeaderValue::from_static("Basic realm=\"fallback\""));
    headers.append("set-cookie", HeaderValue::from_static("session=expired; Path=/"));
    headers.append("set-cookie", HeaderValue::from_static("csrf=cleared; Path=/"));
    headers.insert("x-unselected", HeaderValue::from_static("private"));
    let body = Bytes::from(Vec::from(&b"denied-body"[..]));
    let body_ptr = body.as_ptr() as usize;
    let body_len = body.len();
    let outbound = response(401, headers, body);

    let mut session = MockPluginSession::new();
    let mut seq = Sequence::new();
    session
        .expect_write_response_header()
        .withf(|resp, _| {
            let challenges: Vec<&str> = resp
                .headers
                .get_all("www-authenticate")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .collect();
            let cookies: Vec<&str> = resp
                .headers
                .get_all("set-cookie")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .collect();
            resp.status.as_u16() == 401
                && resp.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("text/plain; charset=utf-8")
                && resp.headers.get("connection").and_then(|v| v.to_str().ok()) == Some("close")
                && resp.headers.get("x-auth-reason").and_then(|v| v.to_str().ok()) == Some("expired")
                && challenges == ["Bearer realm=\"primary\"", "Basic realm=\"fallback\""]
                && cookies == ["session=expired; Path=/", "csrf=cleared; Path=/"]
                && resp.headers.get("x-unselected").is_none()
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(move |body, end_of_stream| {
            *end_of_stream
                && body.as_ref().is_some_and(|body| {
                    body.as_ptr() as usize == body_ptr && body.len() == body_len && body.as_ref() == b"denied-body"
                })
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|| {});

    let mut log = PluginLog::new("ForwardAuth");
    let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
    assert_eq!(
        log.render_for_test(),
        "auth deny upstream_status=401 client_status=401 reason=status_rejected"
    );
}

#[tokio::test]
async fn body_predicate_denial_overrides_status_to_403() {
    let success = SuccessSpec {
        status_codes: None,
        body: vec![BodyPredicate {
            pointer: "/allowed".to_string(),
            op: PredicateOp::Equals(serde_json::Value::Bool(true)),
        }],
    };
    let fa = make_forward_auth_with_success(ForwardAuthDecision::default(), success);

    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    let outbound = response(200, headers, Bytes::from_static(br#"{"allowed":false}"#));

    let mut session = MockPluginSession::new();
    let mut seq = Sequence::new();
    session
        .expect_write_response_header()
        .withf(|resp, _| {
            resp.status.as_u16() == 403
                && resp.headers.get("content-type").and_then(|value| value.to_str().ok()) == Some("application/json")
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(br#"{"allowed":false}"#.as_slice()))
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|| {});

    let mut log = PluginLog::new("ForwardAuth");
    let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
    assert_eq!(
        log.render_for_test(),
        "auth deny upstream_status=200 client_status=403 reason=body_predicate_failed"
    );
}

#[tokio::test]
async fn transport_fail_records_fail_close_before_response_write() {
    let fa = {
        let client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
        client.health.force_unhealthy_for_test();

        ForwardAuth {
            client: Arc::new(client),
            status_on_error: 503,
            decision: ForwardAuthDecision::default(),
            forward_body: false,
        }
    };

    let mut session = make_request_session();
    let mut seq = Sequence::new();
    session
        .expect_record_error()
        .withf(|status| matches!(status, edgion_resources::EdgionErrorCode::ForwardAuthServiceUnreachable))
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| ());
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "auth-server-unreachable")
        .times(1)
        .in_sequence(&mut seq)
        .return_const(());
    session
        .expect_write_response_header()
        .withf(|resp, _| {
            resp.status.as_u16() == 503
                && resp.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("application/json")
                && resp.headers.get("connection").and_then(|v| v.to_str().ok()) == Some("close")
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| {
            *end_of_stream && body.as_deref() == Some(b"{\"message\":\"Auth service unavailable\"}".as_slice())
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|| {});

    let mut log = PluginLog::new("ForwardAuth");
    let result = fa.run_request(&mut session, &mut log).await;

    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
    assert_eq!(log.render_for_test(), "err: upstream unreachable");
}

#[tokio::test]
async fn presence_warning_reaches_plugin_log_without_changing_fail_close() {
    use crate::plugins::http::common::outbound::request::test_presence_failure_request;
    use edgion_resources::resources::common::outbound_request::PresencePolicy;

    for policy in [
        PresencePolicy::WarnIfMissing,
        PresencePolicy::SkipIfMissing,
        PresencePolicy::Required,
    ] {
        let mut client = ManagedOutboundClient::test_with_static_target("http://example.test");
        client.compiled_request = test_presence_failure_request(policy);
        let fa = ForwardAuth {
            client: Arc::new(client),
            status_on_error: 503,
            decision: ForwardAuthDecision::default(),
            forward_body: false,
        };
        let mut session = make_request_session();
        session.expect_get_cookie().return_const(None::<String>);
        session.expect_record_error().times(1).return_const(());
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "auth-server-unreachable")
            .times(1)
            .return_const(());
        session
            .expect_write_response_header()
            .withf(|response, _| response.status.as_u16() == 503)
            .times(1)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(|body, eos| {
                *eos && body.as_deref() == Some(b"{\"message\":\"Auth service unavailable\"}".as_slice())
            })
            .times(1)
            .returning(|_, _| Ok(()));
        session.expect_shutdown().times(1).return_const(());
        let mut log = PluginLog::new("ForwardAuth");
        assert!(matches!(
            fa.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        ));
        let rendered = log.render_for_test();
        assert_eq!(
            rendered.matches("warn outbound field missing").count(),
            usize::from(matches!(policy, PresencePolicy::WarnIfMissing))
        );
        assert!(rendered.ends_with("err: upstream unreachable"));
        assert!(!rendered.contains("private"));
    }
}

#[tokio::test]
async fn auth_deny_does_not_record_outcome() {
    let fa = make_forward_auth(ForwardAuthDecision::default());
    let outbound = response(403, HeaderMap::new(), Bytes::from_static(b"no"));

    let mut session = MockPluginSession::new();
    let mut seq = Sequence::new();
    session
        .expect_write_response_header()
        .withf(|resp, _| resp.status.as_u16() == 403)
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(b"no".as_slice()))
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|| {});

    let mut log = PluginLog::new("ForwardAuth");
    let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

    assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
    assert_eq!(
        log.render_for_test(),
        "auth deny upstream_status=403 client_status=403 reason=status_rejected"
    );
}

#[test]
fn owned_forward_auth_constructor_keeps_owner_namespace_and_decision_contract() {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"auth","namespace":"tenant"},"spec":{"requestPlugins":[{"type":"ForwardAuth","config":{
            "target":{"name":"auth-service","port":8080},"forwardBody":true,"statusOnError":502,"decision":{},
            "retry":{"maxRetries":1},"request":{"path":{"template":"/auth"}}
        }}]}
    }))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let field = entry.managed_http_config().unwrap().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([]);
    let input = PreparedManagedHttp::new(&field, &defaults).unwrap();
    let plugin = ForwardAuth::from_owned(&input).unwrap();
    assert_eq!(plugin.status_on_error, 502);
    assert!(plugin.forward_body);
    assert_eq!(plugin.client.retry.as_ref().unwrap().retry_on_timeout, Some(true));
    assert!(
        matches!(&plugin.client.resolved_target, crate::plugins::http::common::outbound::target::ResolvedTarget::Service { service_key, scheme:"http", .. } if service_key == "tenant/auth-service")
    );
}

#[test]
fn route_forward_auth_constructor_preserves_intrinsic_validation_and_body_retry_contract() {
    use edgion_resources::resources::refs::route_auth_fields::HttpRouteAuthRefs;
    let wire = serde_json::json!({"metadata":{"name":"route","namespace":"tenant"},"spec":{"rules":[{"filters":[{"type":"ExternalAuth","externalAuth":{
        "target":{"name":"auth","port":8080},"decision":{},"forwardBody":true,"statusOnError":502,"retry":{"maxRetries":1},"request":{"path":{"template":"/auth"}}
    }}]}]}});
    for invalid in [false, true] {
        let mut value = wire.clone();
        if invalid {
            value["spec"]["rules"][0]["filters"][0]["externalAuth"]["decision"]["clientHeaders"] =
                serde_json::json!(["Connection"]);
        }
        let route: edgion_resources::HTTPRoute = serde_json::from_value(value).unwrap();
        let references = HttpRouteAuthRefs::from_route(&route).unwrap();
        let field = references.effective_at(0, None, 0).unwrap();
        let defaults = edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([]);
        let input = PreparedRouteAuth::new(&field, &defaults).unwrap();
        let result = ForwardAuth::from_route(&input);
        assert_eq!(result.is_err(), invalid);
        if let Ok(plugin) = result {
            assert!(plugin.forward_body);
            assert_eq!(plugin.status_on_error, 502);
            assert_eq!(plugin.client.retry.as_ref().unwrap().retry_on_timeout, Some(true));
        }
    }
}

#[test]
fn client_header_preparation_preserves_first_spelling_and_other_decision_fields() {
    let config = ForwardAuthConfig {
        decision: ForwardAuthDecision {
            client_headers: vec!["X-First".into(), "X-Second".into(), "x-FIRST".into(), "x-second".into()],
            upstream_headers: vec!["X-User".into(), "x-user".into()],
            ..ForwardAuthDecision::default()
        },
        ..ForwardAuthConfig::default()
    };
    let original = serde_json::to_value(&config).unwrap();
    let client = ManagedOutboundClient::test_with_static_target("http://example.com");
    let prepared = ForwardAuth::from_client(&config, client);
    assert_eq!(prepared.decision.client_headers, ["X-First", "X-Second"]);
    assert_eq!(prepared.decision.upstream_headers, config.decision.upstream_headers);
    let mut expected = config.decision.clone();
    expected.client_headers = vec!["X-First".into(), "X-Second".into()];
    assert_eq!(
        serde_json::to_value(&prepared.decision).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(serde_json::to_value(&config).unwrap(), original);
}
