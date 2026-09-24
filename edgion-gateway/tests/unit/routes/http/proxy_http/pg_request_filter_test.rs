use super::body_declared_snapshot_tests::session_from_request;
use super::*;
use edgion_resources::resources::LocalObjectReference;
use pingora_http::RequestHeader;

#[test]
fn acme_challenge_token_and_host_extracts_token_and_strips_host_port() {
    let mut req = RequestHeader::build("GET", b"/.well-known/acme-challenge/token-1", None).unwrap();
    req.insert_header("host", "example.com:80").unwrap();

    let (token, host) = acme_challenge_token_and_host(&req).unwrap();

    assert_eq!(token, "token-1");
    assert_eq!(host, "example.com");
}

#[test]
fn acme_challenge_token_and_host_strips_port_from_ipv6_literal_host() {
    // A bracketed IPv6 literal with a port must keep its brackets, not be
    // mangled to "[" by a naive split(':') — otherwise the challenge lookup
    // never matches the stored entry.domain.
    let mut req = RequestHeader::build("GET", b"/.well-known/acme-challenge/token-2", None).unwrap();
    req.insert_header("host", "[::1]:80").unwrap();

    let (token, host) = acme_challenge_token_and_host(&req).unwrap();

    assert_eq!(token, "token-2");
    assert_eq!(host, "[::1]");
}

#[test]
fn acme_challenge_token_and_host_ignores_non_challenge_paths() {
    let req = RequestHeader::build("GET", b"/api/.well-known/acme-challenge/token-1", None).unwrap();

    assert!(acme_challenge_token_and_host(&req).is_none());
}

#[test]
fn path_reject_freezes_entry_raw_uri_for_access_log() {
    let mut ctx = EdgionHttpContext::new();
    let cfg = PathNormalizationConfig {
        reject_escaped_slashes: true,
        reject_invalid_percent_encoding: true,
        ..Default::default()
    };

    let status = apply_path_normalization_entry(&mut ctx, "/admin%2Fsecret", Some("trace=1".to_string()), &cfg);

    assert_eq!(status, Some(EdgionErrorCode::PathEscapedSlashRejected));
    assert_eq!(ctx.request_path(), "/admin%2Fsecret");
    assert_eq!(ctx.request_uri(), "/admin%2Fsecret?trace=1");
}

#[test]
fn path_normalization_entry_refreshes_normalized_current_path() {
    let mut ctx = EdgionHttpContext::new();
    let cfg = PathNormalizationConfig::default();

    let status = apply_path_normalization_entry(&mut ctx, "/admin//../secret", None, &cfg);

    assert_eq!(status, None);
    assert_eq!(ctx.request_path(), "/admin//../secret");
    assert_eq!(ctx.path(), "/secret");
}

#[test]
fn cors_extension_ref_scan_skips_missing_ref_and_finds_later_config() {
    let filters = Some(vec![
        HTTPRouteFilter {
            filter_type: HTTPRouteFilterType::ExtensionRef,
            request_header_modifier: None,
            response_header_modifier: None,
            request_redirect: None,
            url_rewrite: None,
            request_mirror: None,
            cors: None,
            external_auth: None,
            extension_ref: None,
        },
        HTTPRouteFilter {
            filter_type: HTTPRouteFilterType::ExtensionRef,
            request_header_modifier: None,
            response_header_modifier: None,
            request_redirect: None,
            url_rewrite: None,
            request_mirror: None,
            cors: None,
            external_auth: None,
            extension_ref: Some(LocalObjectReference {
                group: "edgion.io".to_string(),
                kind: "EdgionPlugins".to_string(),
                name: "cors".to_string(),
                resolved_namespace: None,
            }),
        },
    ]);
    let mut runtime = crate::plugins::runtime::PluginRuntime::new();
    runtime.add_from_request_filters(
        &[edgion_resources::resources::RequestFilterEntry::new(
            edgion_resources::resources::EdgionPlugin::Cors(CorsConfig::default()),
        )],
        "default",
        "default/cors",
    );
    let runtime = Arc::new(runtime);
    let expected = runtime.cors_config_arc().expect("Cors config must be cached");

    let actual = extract_cors_config_from_route_filters(&filters, &[Some(runtime)]);

    assert!(actual.is_some_and(|config| Arc::ptr_eq(&config, &expected)));
}

#[tokio::test]
async fn invalid_route_runtime_gate_terminates_options_before_preflight() {
    let filters: Vec<HTTPRouteFilter> = serde_json::from_value(serde_json::json!([
        {"type": "ExtensionRef"}
    ]))
    .expect("HTTP filter fixture must deserialize");
    let runtime = crate::plugins::runtime::PluginRuntime::from_httproute_filters_for_rule(
        &filters,
        "default",
        &Default::default(),
    );
    let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
    let mut ctx = EdgionHttpContext::default();

    runtime.run_preflight_gate(&mut session, &mut ctx).await;

    assert!(ctx.plugin_terminated());
}
