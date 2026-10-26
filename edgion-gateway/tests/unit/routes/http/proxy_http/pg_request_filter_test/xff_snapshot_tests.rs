use super::*;
use crate::observe::AccessLogger;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use crate::util::RealIpExtractor;
use edgion_resources::path_normalization::PathNormalizationConfig;
use edgion_resources::{EdgionGatewayConfig, ListenerKey};
use http::HeaderValue;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, DuplexStream};

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
    RealIpExtractor::new(header.parse().unwrap(), true, None)
}

fn trusted(ip: &std::net::IpAddr) -> bool {
    matches!(ip, std::net::IpAddr::V4(ip) if ip.is_private() || ip.is_unspecified())
}

fn trusted_proxy_policy(header: &str) -> crate::plugins::http::real_ip::RealIpPolicy {
    let owner: EdgionGatewayConfig = serde_json::from_value(json!({
        "metadata": {"name": "real-ip"}, "spec": {"realIp": {"trustedIpRefs": [{"name": "proxies", "namespace": "default"}], "realIpHeader": header}}
    })).unwrap();
    let compiler =
        crate::plugins::runtime::conditions::ip_refs::IpConditionCompiler::from_gateway_config(&owner).unwrap();
    crate::plugins::http::real_ip::RealIpPolicy::bind(owner.spec.real_ip.as_ref().unwrap(), &compiler).unwrap()
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
    for (fields, limit, expected, too_long) in [
        (vec![], Some(0), None, false),
        (vec![b"".as_slice()], Some(0), Some(""), false),
        (vec![b"".as_slice(), b""], Some(1), None, true),
        (vec![b"".as_slice(), b""], Some(2), Some(", "), false),
        (vec![b"abc".as_slice()], Some(3), Some("abc"), false),
        (vec![b"abc".as_slice()], Some(2), None, true),
        (vec![b"\xff".as_slice(), b"12345"], Some(7), None, true),
        (vec![b"12345".as_slice(), b"\xff"], Some(4), None, true),
        (vec![b"a".as_slice(), b"bc", b"\xff"], Some(4), None, true),
        (vec![b"\xff".as_slice()], Some(1), None, false),
        (vec![b"a".as_slice(), b"bc"], None, Some("a, bc"), false),
        (vec![b"a".as_slice(), b"\xff"], None, None, false),
    ] {
        let values: Vec<_> = fields
            .iter()
            .map(|field| HeaderValue::from_bytes(field).unwrap())
            .collect();
        let request = request_with_xff(&values);
        let snapshot = snapshot_x_forwarded_for(&request, limit);
        assert_eq!(
            snapshot.value.as_deref(),
            expected,
            "fields={fields:?}, limit={limit:?}"
        );
        assert_eq!(snapshot.too_long, too_long, "fields={fields:?}, limit={limit:?}");
    }

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
    assert_eq!(xff.real_ip_header().as_str(), "x-forwarded-for");
    assert_eq!(
        xff.resolve_real_ip("192.168.1.4:8080", snapshot.value.as_deref(), trusted)
            .remote_ip
            .to_string(),
        "203.0.113.7"
    );

    let custom = trusted_proxy_extractor("X-Client-IP");
    assert_ne!(custom.real_ip_header().as_str(), "x-forwarded-for");
    assert_eq!(
        custom
            .resolve_real_ip(
                "192.168.1.4:8080",
                request.headers.get("x-client-ip").and_then(|v| v.to_str().ok()),
                trusted
            )
            .remote_ip
            .to_string(),
        "198.51.100.9"
    );

    let invalid = request_with_xff(&[HeaderValue::from_bytes(b"\xff").unwrap()]);
    let snapshot = snapshot_x_forwarded_for(&invalid, Some(200));
    assert_eq!(
        xff.resolve_real_ip("192.168.1.4:8080", snapshot.value.as_deref(), trusted)
            .remote_ip
            .to_string(),
        "192.168.1.4"
    );
}

#[tokio::test]
async fn duplicate_host_is_rejected_during_protocol_parsing() {
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: first.test\r\nHost: second.test\r\nX-Forwarded-For: 12345\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.is_err());
}

#[tokio::test]
async fn absolute_form_entry_uses_path_for_matching_and_keeps_raw_target() {
    let (mut session, _client) =
        session_from_request(b"GET http://example.test/%61pi?scope=read HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
    let mut ctx = context(200);

    assert!(!build_request_metadata(&proxy(), &mut session, &mut ctx).await.unwrap());
    assert_eq!(session.req_header().raw_path(), b"http://example.test/%61pi?scope=read");
    assert_eq!(ctx.request_info.hostname.as_str(), "example.test");
    assert_eq!(ctx.request_path(), "/%61pi");
    assert_eq!(ctx.path(), "/api");
    assert_eq!(ctx.request_uri(), "/%61pi?scope=read");
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
    )
    .unwrap();
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
        real_ip_policy: Some(Arc::new(trusted_proxy_policy("X-Forwarded-For"))),
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
