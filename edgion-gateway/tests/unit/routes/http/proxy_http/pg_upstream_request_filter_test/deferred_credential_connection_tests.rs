use super::{local_pre_dispatch_failure, upstream_request_filter, validate_deferred_credential_connection};
use crate::ctx::DeferredCredentialMutations;
use crate::observe::AccessLogger;
use crate::plugins::http::compile_plugins;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::proxy_http::{EdgionHttpProxy, PreflightHandler};
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use crate::EdgionHttpContext;
use edgion_resources::{
    EdgionErrorCode, EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, ListenerKey, RequestFilterEntry,
};
use http::{HeaderName, HeaderValue};
use pingora_core::ErrorType;
use pingora_http::RequestHeader;
use pingora_proxy::Session;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

fn mutations() -> DeferredCredentialMutations {
    let mut mutations = DeferredCredentialMutations::default();
    mutations.upsert(
        HeaderName::from_static("x-provider-token"),
        Arc::new(HeaderValue::from_static("private-token")),
        None,
    );
    mutations
}

#[test]
fn connection_nomination_checks_borrow_tokens_without_allocating() {
    let mutations = mutations();
    let mut session_request = RequestHeader::build("GET", b"/", None).unwrap();
    session_request
        .append_header("connection", " , Keep-Alive, keep-alive, , X-Other, ")
        .unwrap();
    session_request.append_header("connection", "Upgrade").unwrap();
    let mut attempt = RequestHeader::build("GET", b"/", None).unwrap();
    attempt.insert_header("connection", "X-Other, x-other,,").unwrap();

    for nominated in [false, true] {
        if nominated {
            attempt
                .append_header("connection", " , x-PrOvIdEr-ToKeN, X-Provider-Token, ")
                .unwrap();
        }
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            validate_deferred_credential_connection(
                std::hint::black_box(&session_request),
                std::hint::black_box(&attempt),
                &mutations,
            )
        });
        assert_eq!(result.is_err(), nominated);
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn rejects_target_nominated_by_a_late_connection_mutation() {
    let session_request = RequestHeader::build("GET", b"/", None).unwrap();
    let mut request = RequestHeader::build("GET", b"/", None).unwrap();
    request
        .insert_header("connection", "keep-alive, X-Provider-Token")
        .unwrap();
    assert!(validate_deferred_credential_connection(&session_request, &request, &mutations()).is_err());
}

#[test]
fn accepts_unrelated_options_and_rejects_non_utf8_connection_values() {
    let session_request = RequestHeader::build("GET", b"/", None).unwrap();
    let mut unrelated = RequestHeader::build("GET", b"/", None).unwrap();
    unrelated.insert_header("connection", "keep-alive, X-Other").unwrap();
    assert!(validate_deferred_credential_connection(&session_request, &unrelated, &mutations()).is_ok());

    let mut invalid = RequestHeader::build("GET", b"/", None).unwrap();
    invalid
        .insert_header("connection", HeaderValue::from_bytes(b"X-Other, \xff").unwrap())
        .unwrap();
    assert!(validate_deferred_credential_connection(&session_request, &invalid, &mutations()).is_err());
}

#[test]
fn checks_every_field_case_insensitively_and_accepts_upgrade() {
    let mut session_request = RequestHeader::build("GET", b"/", None).unwrap();
    session_request.append_header("connection", "keep-alive").unwrap();
    session_request
        .append_header("connection", "X-OTHER, x-PrOvIdEr-ToKeN")
        .unwrap();
    let mut attempt = RequestHeader::build("GET", b"/", None).unwrap();
    attempt.insert_header("connection", "Upgrade").unwrap();
    attempt.insert_header("upgrade", "websocket").unwrap();

    assert!(validate_deferred_credential_connection(&session_request, &attempt, &mutations()).is_err());

    session_request.remove_header(&http::header::CONNECTION);
    session_request
        .insert_header("connection", "keep-alive, X-Other")
        .unwrap();
    assert!(validate_deferred_credential_connection(&session_request, &attempt, &mutations()).is_ok());
}

#[test]
fn rejects_non_utf8_in_the_session_request() {
    let mut session_request = RequestHeader::build("GET", b"/", None).unwrap();
    session_request
        .insert_header("connection", HeaderValue::from_bytes(b"X-Other, \xff").unwrap())
        .unwrap();
    let attempt = RequestHeader::build("GET", b"/", None).unwrap();
    assert!(validate_deferred_credential_connection(&session_request, &attempt, &mutations()).is_err());
}

#[test]
fn local_pre_dispatch_error_is_static_500_and_nonretryable() {
    let error = local_pre_dispatch_failure();
    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
}

fn proxy() -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: ListenerKey("127.0.0.1:80/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: PreflightHandler,
        http_route_manager: Arc::new(HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
    }
}

async fn session_from_request(raw_request: &[u8]) -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(raw_request).await.unwrap();
    drop(client);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    session.read_request().await.unwrap();
    session
}

#[tokio::test]
async fn full_hook_rejects_session_nomination_before_materialization_or_ai_dispatch() {
    let mut session =
        session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\nConnection: keep-alive, X-Provider-Token\r\n\r\n").await;
    let mut attempt = session.req_header().clone();
    attempt.remove_header(&http::header::CONNECTION);
    let mut ctx = EdgionHttpContext::default();
    ctx.upsert_deferred_credential_mutation(
        HeaderName::from_static("x-provider-token"),
        Arc::new(HeaderValue::from_static("private-token")),
        None,
    );

    let error = upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
        .await
        .unwrap_err();

    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::CredentialConnectionConflict));
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::CredentialExpired));
    assert!(!attempt.headers.contains_key("x-provider-token"));
    assert!(ctx
        .ai_state()
        .is_none_or(|state| !state.attempt_runtime.ever_dispatched()));
}

#[tokio::test]
async fn full_hook_rejects_attempt_local_nomination() {
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut attempt = session.req_header().clone();
    attempt.insert_header("connection", "x-provider-token").unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.upsert_deferred_credential_mutation(
        HeaderName::from_static("x-provider-token"),
        Arc::new(HeaderValue::from_static("private-token")),
        None,
    );

    let error = upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
        .await
        .unwrap_err();
    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::CredentialConnectionConflict));
    assert!(!attempt.headers.contains_key("x-provider-token"));
}

#[tokio::test]
async fn full_hook_preserves_expiry_diagnostic_without_materializing_secret() {
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut attempt = session.req_header().clone();
    let mut ctx = EdgionHttpContext::default();
    ctx.upsert_deferred_credential_mutation(
        HeaderName::from_static("x-provider-token"),
        Arc::new(HeaderValue::from_static("private-token")),
        Some(std::time::Instant::now()),
    );

    let error = upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
        .await
        .unwrap_err();

    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::CredentialExpired));
    assert!(!ctx
        .diagnostics
        .contains_error(EdgionErrorCode::CredentialConnectionConflict));
    assert!(!attempt.headers.contains_key("x-provider-token"));
}

#[tokio::test]
async fn full_hook_without_credentials_completes_local_preparation() {
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut attempt = session.req_header().clone();
    let mut ctx = EdgionHttpContext::default();

    upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
        .await
        .unwrap();

    assert!(ctx.diagnostics.is_empty());
    assert!(ctx.deferred_credential_mutations().is_none());
    assert!(ctx
        .ai_state()
        .is_none_or(|state| !state.attempt_runtime.ever_dispatched()));
}

#[tokio::test]
async fn full_hook_materializes_a_valid_unexpired_credential() {
    let mut session = session_from_request(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut attempt = session.req_header().clone();
    let mut ctx = EdgionHttpContext::default();
    ctx.upsert_deferred_credential_mutation(
        HeaderName::from_static("x-provider-token"),
        Arc::new(HeaderValue::from_static("private-token")),
        Some(std::time::Instant::now() + std::time::Duration::from_secs(60)),
    );

    upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
        .await
        .unwrap();

    assert_eq!(attempt.headers.get("x-provider-token").unwrap(), "private-token");
    assert!(ctx.diagnostics.is_empty());
}

#[tokio::test]
async fn full_hook_keeps_forwarded_identity_authoritative_over_request_ids_on_every_attempt() {
    let mut session = session_from_request(
        b"GET / HTTP/1.1\r\nHost: test\r\n\
              Traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01\r\n\
              X-Real-IP: client-correlation\r\n\
              X-Business-ID: business-42\r\n\
              X-Forwarded-For: 198.51.100.8\r\n\r\n",
    )
    .await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    ctx.request_info.trace_id.reconcile_request(session.req_header_mut());

    let config = serde_json::from_value(serde_json::json!({
        "ids": [
            {"header": "x-real-ip", "generateWhenMissing": false},
            {
                "header": "x-order-id",
                "source": {"type": "header", "name": "x-business-id"},
                "generateWhenMissing": false
            }
        ]
    }))
    .unwrap();
    let resource = EdgionPlugins::new(
        "forwarded-identity-collision",
        EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestId(config))]),
            ..Default::default()
        },
    );
    let compiled = compile_plugins(&resource);
    assert!(compiled.is_activatable);
    compiled
        .plugin_runtime
        .run_request_plugins(&mut session, &mut ctx)
        .await;

    let trace_id = ctx.request_info.trace_id.standard_trace_id().unwrap().to_owned();
    assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(
        ctx.request_info.trace_id.correlation_id("x-real-ip"),
        Some("client-correlation")
    );
    assert_eq!(
        ctx.request_info.trace_id.correlation_id("x-order-id"),
        Some("business-42")
    );

    ctx.request_info.x_forwarded_for = Some("198.51.100.8".to_string());
    ctx.request_info.downstream_peer_ip = "10.0.0.5".to_string();
    ctx.request_info.remote_ip = "203.0.113.9".to_string();

    for _ in 0..2 {
        let mut attempt = session.req_header().clone();
        upstream_request_filter(&proxy(), &mut session, &mut attempt, &mut ctx)
            .await
            .unwrap();

        assert_eq!(attempt.headers.get("x-real-ip").unwrap(), "203.0.113.9");
        assert_eq!(
            attempt.headers.get("x-forwarded-for").unwrap(),
            "198.51.100.8, 10.0.0.5"
        );
        assert_eq!(attempt.headers.get("x-order-id").unwrap(), "business-42");
        assert_eq!(attempt.headers.get("x-trace-id").unwrap(), trace_id.as_str());
        assert_eq!(
            attempt.headers.get("traceparent").unwrap(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
    }

    assert_eq!(
        ctx.request_info.trace_id.correlation_id("x-real-ip"),
        Some("client-correlation")
    );
    assert_eq!(
        ctx.request_info.trace_id.correlation_id("x-order-id"),
        Some("business-42")
    );
    let access_log_ids = serde_json::to_value(&ctx.request_info.trace_id).unwrap();
    assert_eq!(ctx.request_info.trace_id.standard_trace_id(), Some(trace_id.as_str()));
    assert_eq!(access_log_ids["trace_id"], trace_id);
    assert_eq!(access_log_ids["request_ids"]["x-real-ip"], "client-correlation");
    assert_eq!(access_log_ids["request_ids"]["x-order-id"], "business-42");
    assert!(ctx.diagnostics.is_empty());
    assert!(ctx
        .ai_state()
        .is_none_or(|state| !state.attempt_runtime.ever_dispatched()));
}
