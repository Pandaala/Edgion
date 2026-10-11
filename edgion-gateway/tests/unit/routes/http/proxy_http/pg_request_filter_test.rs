use super::body_declared_snapshot_tests::session_from_request;
use super::*;
use edgion_resources::resources::{HTTPRouteFilter, LocalObjectReference};
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
            resolved_external_auth: None,
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
            resolved_external_auth: None,
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

    // The malformed slot retains its ordinal; the separate runtime test proves rejection.
    assert_eq!(filters.as_ref().unwrap().len(), 2);
    let actual = extract_cors_config_from_route_filters(&[None, Some(runtime)]);

    assert!(actual.is_some_and(|config| Arc::ptr_eq(&config, &expected)));
}

#[tokio::test]
async fn invalid_route_runtime_gate_terminates_options_before_preflight() {
    let filters: Vec<HTTPRouteFilter> = serde_json::from_value(serde_json::json!([
        {"type": "ExtensionRef"}
    ]))
    .expect("HTTP filter fixture must deserialize");
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "route", "namespace": "default"}, "spec": {"rules": [{"filters": filters}]}
    }))
    .unwrap();
    let runtime =
        crate::plugins::runtime::PluginRuntime::from_httproute_filters_for_rule(&route, 0, &Default::default());
    let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
    let mut ctx = EdgionHttpContext::default();

    runtime.run_preflight_gate(&mut session, &mut ctx).await;

    assert!(ctx.plugin_terminated());
}

#[tokio::test]
async fn invalid_extension_after_native_cors_uses_original_preflight_error_entry() {
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"route","namespace":"default"}, "spec":{"rules":[{"filters":[
            {"type":"CORS","cors":{"allowOrigins":["https://app.example"],"allowMethods":["GET"]}},
            {"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"bad/name"}}
        ]}]}
    }))
    .unwrap();
    let runtime =
        crate::plugins::runtime::PluginRuntime::from_httproute_filters_for_rule(&route, 0, &Default::default());
    assert_eq!(runtime.request_plugins_count(), 2);
    assert_eq!(runtime.ext_ref_keys(), &[None]);
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut client, server) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(server));
    client.write_all(b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n").await.unwrap();
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::default();
    runtime.run_preflight_gate(&mut session, &mut ctx).await;
    assert!(ctx.plugin_terminated());
    assert_eq!(session.response_written().unwrap().status.as_u16(), 500);
    let mut response = [0; 4096];
    let received = client.read(&mut response).await.unwrap();
    assert!(std::str::from_utf8(&response[..received])
        .unwrap()
        .contains(r#"{"message":"plugin reference error"}"#));
    let logs = serde_json::to_string(ctx.stage_logs()).unwrap();
    assert!(logs.contains("ExtensionRef"));
    assert!(logs.contains("extension ref target not supported"));
}

#[test]
fn invalid_same_name_cold_key_cannot_select_cors_or_waf_decoy() {
    use crate::plugins::http::PluginStore;
    use crate::plugins::runtime::PluginRuntime;
    use crate::routes::http::proxy_http::global_plugins_resolve::{resolve_route_ext_refs, select_waf_owner};
    use std::collections::HashMap;
    let mut plugin: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"decoy","namespace":"default"}, "spec":{"requestPlugins":[{"type":"Cors","config":{}}]}
    }))
    .unwrap();
    plugin.spec.waf = Some(crate::plugins::http::waf::fixture::config_in_namespace(
        "SecAction \"id:1,phase:1,pass\"\n",
        "default",
    ));
    let store = PluginStore::new();
    store.prepare_test_scope(std::collections::HashSet::from(["default/decoy".into()]));
    store
        .publisher()
        .replace_http_plugins(HashMap::from([("default/decoy".into(), Arc::new(plugin))]));
    assert!(store.scope().http("default/decoy").unwrap().plugin_runtime.has_waf());
    for (group, present) in [("", false), ("edgion.io", true)] {
        let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"route","namespace":"default"}, "spec":{"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":group,"kind":"EdgionPlugins","name":"decoy"}}]}]}
        })).unwrap();
        let runtime = PluginRuntime::from_httproute_filters_for_rule(&route, 0, &Default::default());
        let resolved = resolve_route_ext_refs(&store.scope(), runtime.ext_ref_keys());
        assert_eq!(resolved.len(), 1);
        assert_eq!(extract_cors_config_from_route_filters(&resolved).is_some(), present);
        assert_eq!(select_waf_owner(&[], &[], &resolved).is_some(), present);
    }
}

#[test]
fn body_composition_covers_route_refs_and_unselected_backends() {
    fn native(filters: serde_json::Value) -> PluginRuntime {
        let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"route","namespace":"apps"},"spec":{"rules":[{"filters":filters}]}
        }))
        .unwrap();
        PluginRuntime::from_httproute_filters_for_rule(&route, 0, &Default::default())
    }
    let mut waf = PluginRuntime::new();
    waf.add_waf(&crate::plugins::http::waf::fixture::config(
        "SecAction \"id:1,phase:1,pass\"\n",
    ))
    .unwrap();
    let waf = Arc::new(waf);
    let route = native(
        serde_json::json!([{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"waf"}}]),
    );
    let refs = [Some(Arc::clone(&waf))];
    let empty_backend = PluginRuntime::new();
    assert!(allows_partial_body_capture(
        &[],
        &[],
        &refs,
        &route,
        [&empty_backend].into_iter()
    ));
    assert!(!allows_partial_body_capture(
        &[],
        &[],
        &[None],
        &route,
        std::iter::empty()
    ));
    assert!(!allows_partial_body_capture(&[], &[], &[], &route, std::iter::empty()));
    let modifier = Arc::new(native(
        serde_json::json!([{"type":"RequestHeaderModifier","requestHeaderModifier":{"set":[{"name":"x-check","value":"1"}]}}]),
    ));
    assert!(!allows_partial_body_capture(
        &[Arc::clone(&modifier)],
        &[],
        &refs,
        &route,
        std::iter::empty()
    ));
    assert!(!allows_partial_body_capture(
        &[],
        &[Arc::clone(&modifier)],
        &refs,
        &route,
        std::iter::empty()
    ));
    assert!(!allows_partial_body_capture(
        &[],
        &[],
        &refs,
        &route,
        [&empty_backend, modifier.as_ref()].into_iter()
    ));
    assert!(!allows_partial_body_capture(
        &[],
        &[],
        &[],
        &modifier,
        std::iter::empty()
    ));
    assert!(!EdgionHttpContext::default().waf_partial_capture_allowed);
}
