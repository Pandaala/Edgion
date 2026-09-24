use super::*;
use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK;
use crate::observe::AccessLogger;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use crate::util::RealIpExtractor;
use edgion_resources::path_normalization::PathNormalizationConfig;
use edgion_resources::resources::edgion_config_data::IpGroup;
use edgion_resources::{EdgionGatewayConfig, ListenerKey};
use http::HeaderValue;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

fn request_with_xff(values: &[HeaderValue]) -> RequestHeader {
    let mut request = RequestHeader::build("GET", b"/", None).unwrap();
    request.insert_header("host", "example.test").unwrap();
    for value in values {
        request.append_header("x-forwarded-for", value).unwrap();
    }
    request
}

fn context(limit: usize) -> EdgionHttpContext {
    let config: EdgionGatewayConfig = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionGatewayConfig",
        "metadata": {"name": "xff-test"},
        "spec": {"securityProtect": {"xForwardedForLimit": limit}}
    }))
    .unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.egc = Arc::new(crate::config::ResolvedGatewayConfig::resolve(&config));
    ctx
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
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
    }
}

async fn session_from_request(raw_request: &[u8]) -> (Session, DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(raw_request).await.unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    assert!(session.read_request().await.unwrap());
    (session, client)
}

fn trusted_proxy_extractor(header: &str) -> RealIpExtractor {
    RealIpExtractor::new(
        &[IpGroup {
            name: "trusted".to_string(),
            description: None,
            cidrs: vec![
                "0.0.0.0/32".to_string(),
                "192.168.0.0/16".to_string(),
                "10.0.0.0/8".to_string(),
            ],
        }],
        header.to_string(),
        true,
        None,
    )
    .unwrap()
}

#[test]
fn snapshot_handles_missing_single_repeated_and_empty_field_lines() {
    let missing = request_with_xff(&[]);
    let snapshot = snapshot_x_forwarded_for(&missing, Some(200));
    assert_eq!(snapshot.value, None);
    assert!(!snapshot.too_long);

    let single = request_with_xff(&[HeaderValue::from_static("203.0.113.7")]);
    let snapshot = snapshot_x_forwarded_for(&single, Some(200));
    assert_eq!(snapshot.value.as_deref(), Some("203.0.113.7"));
    assert!(!snapshot.too_long);

    let repeated = request_with_xff(&[
        HeaderValue::from_static(""),
        HeaderValue::from_static("203.0.113.7"),
        HeaderValue::from_static("10.0.0.5"),
        HeaderValue::from_static(""),
    ]);
    let snapshot = snapshot_x_forwarded_for(&repeated, Some(200));
    assert_eq!(snapshot.value.as_deref(), Some(", 203.0.113.7, 10.0.0.5, "));
    assert!(!snapshot.too_long);
}

#[test]
fn snapshot_limit_counts_separators_and_invalid_lines_cannot_bypass_it() {
    let repeated = request_with_xff(&[HeaderValue::from_static("a"), HeaderValue::from_static("bc")]);
    let exact = snapshot_x_forwarded_for(&repeated, Some(5));
    assert_eq!(exact.value.as_deref(), Some("a, bc"));
    assert!(!exact.too_long);
    let over = snapshot_x_forwarded_for(&repeated, Some(4));
    assert_eq!(over.value, None);
    assert!(over.too_long);

    let invalid = request_with_xff(&[
        HeaderValue::from_bytes(b"\xff").unwrap(),
        HeaderValue::from_static("12345"),
    ]);
    let invalid_exact = snapshot_x_forwarded_for(&invalid, Some(8));
    assert_eq!(invalid_exact.value, None);
    assert!(!invalid_exact.too_long);
    let invalid_over = snapshot_x_forwarded_for(&invalid, Some(7));
    assert_eq!(invalid_over.value, None);
    assert!(invalid_over.too_long);
}

#[test]
fn configured_xff_is_case_insensitive_while_custom_header_keeps_its_path() {
    let mut request = request_with_xff(&[
        HeaderValue::from_static("203.0.113.7"),
        HeaderValue::from_static("10.0.0.5"),
    ]);
    request.insert_header("x-client-ip", "198.51.100.9").unwrap();
    let snapshot = snapshot_x_forwarded_for(&request, Some(200));

    let xff = trusted_proxy_extractor("X-FoRwArDeD-FoR");
    assert!(xff.real_ip_header().eq_ignore_ascii_case("x-forwarded-for"));
    assert_eq!(
        xff.resolve_real_ip("192.168.1.4:8080", snapshot.value.as_deref())
            .remote_ip,
        "203.0.113.7"
    );

    let custom = trusted_proxy_extractor("X-Client-IP");
    assert!(!custom.real_ip_header().eq_ignore_ascii_case("x-forwarded-for"));
    assert_eq!(custom.extract_real_ip("192.168.1.4:8080", &request), "198.51.100.9");

    let invalid = request_with_xff(&[HeaderValue::from_bytes(b"\xff").unwrap()]);
    let snapshot = snapshot_x_forwarded_for(&invalid, Some(200));
    assert_eq!(
        xff.resolve_real_ip("192.168.1.4:8080", snapshot.value.as_deref())
            .remote_ip,
        "192.168.1.4"
    );
}

#[tokio::test]
async fn duplicate_host_precedes_xff_rejection_without_publishing_xff_metadata() {
    let (mut session, _client) =
        session_from_request(b"GET / HTTP/1.1\r\nHost: first.test\r\nX-Forwarded-For: 12345\r\n\r\n").await;
    session.req_header_mut().append_header("host", "second.test").unwrap();
    let mut ctx = context(4);

    assert!(build_request_metadata(&proxy(), &mut session, &mut ctx).await.unwrap());
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::DuplicateHostHeader));
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::XffHeaderTooLong));
    assert_eq!(ctx.request_info.x_forwarded_for, None);
}

#[tokio::test]
async fn duplicate_host_gate_uses_the_request_pinned_egc_generation() {
    let (proxy, mut pinned_true, mut updated_false) = {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
        let config = |reject_duplicate_host| {
            serde_json::from_value::<EdgionGatewayConfig>(json!({
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionGatewayConfig",
                "metadata": {"name": "duplicate-host-policy"},
                "spec": {"securityProtect": {"rejectDuplicateHost": reject_duplicate_host}}
            }))
            .unwrap()
        };
        handler.full_set(&HashMap::from([(
            "duplicate-host-policy".to_string(),
            Arc::new(config(true)),
        )]));
        let mut proxy = proxy();
        proxy.egc = crate::config::bind_gateway_config(Some("duplicate-host-policy"));
        let pinned_true = crate::routes::http::proxy_http::pg_new_ctx::new_ctx(&proxy);

        handler.partial_update(
            HashMap::new(),
            HashMap::from([("duplicate-host-policy".to_string(), Arc::new(config(false)))]),
            HashSet::new(),
        );
        let updated_false = crate::routes::http::proxy_http::pg_new_ctx::new_ctx(&proxy);
        handler.full_set(&HashMap::new());
        (proxy, pinned_true, updated_false)
    };

    let (mut rejected, mut rejected_wire) = session_from_request(b"GET / HTTP/1.1\r\nHost: first.test\r\n\r\n").await;
    rejected.req_header_mut().append_header("host", "second.test").unwrap();
    assert!(build_request_metadata(&proxy, &mut rejected, &mut pinned_true)
        .await
        .unwrap());
    assert!(pinned_true
        .diagnostics
        .contains_error(EdgionErrorCode::DuplicateHostHeader));
    drop(rejected);
    let mut response = String::new();
    rejected_wire.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 400"));

    let (mut allowed, _allowed_wire) = session_from_request(b"GET / HTTP/1.1\r\nHost: first.test\r\n\r\n").await;
    allowed.req_header_mut().append_header("host", "second.test").unwrap();
    assert!(!build_request_metadata(&proxy, &mut allowed, &mut updated_false)
        .await
        .unwrap());
    assert!(!updated_false
        .diagnostics
        .contains_error(EdgionErrorCode::DuplicateHostHeader));
    assert_eq!(updated_false.request_info.hostname.as_str(), "first.test");
}

#[tokio::test]
async fn missing_host_precedes_xff_rejection() {
    let (mut session, _client) = session_from_request(b"GET / HTTP/1.1\r\nX-Forwarded-For: 12345\r\n\r\n").await;
    let mut ctx = context(4);

    assert!(build_request_metadata(&proxy(), &mut session, &mut ctx).await.unwrap());
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::HostMissing));
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::XffHeaderTooLong));
    assert_eq!(ctx.request_info.x_forwarded_for, None);
}

#[test]
fn production_validation_seam_prioritizes_path_over_pending_xff_rejection() {
    let request = request_with_xff(&[HeaderValue::from_static("12345")]);
    let snapshot = snapshot_x_forwarded_for(&request, Some(4));
    assert!(snapshot.too_long);

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.hostname = NormalizedHostname::new("example.test");
    let error = request_metadata_validation_error(
        &mut ctx,
        "/admin%2Fsecret",
        None,
        &PathNormalizationConfig {
            reject_escaped_slashes: true,
            ..Default::default()
        },
        snapshot.too_long,
    );
    assert_eq!(error, Some(EdgionErrorCode::PathEscapedSlashRejected));
}

#[tokio::test]
async fn invalid_under_limit_xff_falls_back_and_upstream_replaces_it_with_peer() {
    let (mut session, _client) = session_from_request(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
    session
        .req_header_mut()
        .append_header("x-forwarded-for", HeaderValue::from_bytes(b"\xff").unwrap())
        .unwrap();
    let proxy = proxy();
    let mut ctx = context(200);
    let config = crate::config::ResolvedGatewayConfig {
        real_ip_extractor: Some(Arc::new(trusted_proxy_extractor("X-Forwarded-For"))),
        ..Default::default()
    };
    ctx.egc = Arc::new(config);

    assert!(!build_request_metadata(&proxy, &mut session, &mut ctx).await.unwrap());
    assert_eq!(ctx.request_info.remote_ip, "0.0.0.0");
    assert_eq!(ctx.request_info.x_forwarded_for, None);
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::XffHeaderTooLong));

    let expected_peer = ctx.request_info.downstream_peer_ip.clone();
    let mut upstream = session.req_header().clone();
    super::super::pg_upstream_request_filter::upstream_request_filter(&proxy, &mut session, &mut upstream, &mut ctx)
        .await
        .unwrap();
    assert_eq!(
        upstream.headers.get("x-forwarded-for").unwrap().to_str().unwrap(),
        expected_peer
    );
}

#[tokio::test]
async fn invalid_overlimit_xff_is_rejected_and_valid_repeated_xff_reaches_upstream() {
    let (mut invalid_session, _client) = session_from_request(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
    invalid_session
        .req_header_mut()
        .append_header("x-forwarded-for", HeaderValue::from_bytes(b"\xff").unwrap())
        .unwrap();
    invalid_session
        .req_header_mut()
        .append_header("x-forwarded-for", "12345")
        .unwrap();
    let mut invalid_ctx = context(7);
    assert!(build_request_metadata(&proxy(), &mut invalid_session, &mut invalid_ctx)
        .await
        .unwrap());
    assert!(invalid_ctx
        .diagnostics
        .contains_error(EdgionErrorCode::XffHeaderTooLong));
    assert_eq!(invalid_ctx.request_info.x_forwarded_for, None);

    let (mut session, _client) = session_from_request(
        b"GET / HTTP/1.1\r\nHost: example.test\r\nX-Forwarded-For: 203.0.113.7\r\nX-Forwarded-For: 10.0.0.5\r\n\r\n",
    )
    .await;
    let proxy = proxy();
    let mut ctx = EdgionHttpContext::default();
    assert!(!build_request_metadata(&proxy, &mut session, &mut ctx).await.unwrap());
    assert_eq!(
        ctx.request_info.x_forwarded_for.as_deref(),
        Some("203.0.113.7, 10.0.0.5")
    );

    let mut upstream = session.req_header().clone();
    super::super::pg_upstream_request_filter::upstream_request_filter(&proxy, &mut session, &mut upstream, &mut ctx)
        .await
        .unwrap();
    let expected = format!("203.0.113.7, 10.0.0.5, {}", ctx.request_info.downstream_peer_ip);
    assert_eq!(
        upstream.headers.get("x-forwarded-for").unwrap().to_str().unwrap(),
        expected
    );
}
