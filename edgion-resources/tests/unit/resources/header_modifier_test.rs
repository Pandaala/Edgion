use super::*;
use serde_json::{json, Value};

fn assert_config(config: Value, valid: bool) {
    let http: HTTPHeaderFilter = serde_json::from_value(config.clone()).unwrap();
    let grpc: GRPCHeaderFilter = serde_json::from_value(config.clone()).unwrap();
    assert_eq!(http.validate().is_ok(), valid, "HTTP config: {config}");
    assert_eq!(grpc.validate(), http.validate(), "HTTP/gRPC parity: {config}");
}

#[test]
fn header_modifier_names_and_values_use_shared_http_grpc_contract() {
    for name in [
        "",
        "X Tenant",
        ":authority",
        "x:bad",
        "x\rname",
        "x\nname",
        "x\0name",
        "é",
    ] {
        for field in ["set", "add"] {
            assert_config(json!({field: [{"name": name, "value": "valid"}]}), false);
        }
        assert_config(json!({"remove": [name]}), false);
    }
    assert_config(json!({"remove": ["x".repeat(257)]}), false);
    assert_config(json!({"set": [{"name": "x".repeat(257), "value": "valid"}]}), false);
    for value in [
        "",
        "line\rbreak",
        "line\nbreak",
        "nul\0byte",
        "del\u{7f}",
        "control\u{1f}",
    ] {
        for field in ["set", "add"] {
            assert_config(json!({field: [{"name": "x-test", "value": value}]}), false);
        }
    }
    assert_config(json!({"add": [{"name": "x-test", "value": "x".repeat(4097)}]}), false);
    assert_config(json!({"set": [{"name": "x-test", "value": "é".repeat(4097)}]}), false);
}

#[test]
fn header_modifier_accepts_standard_boundaries_and_literal_values() {
    assert_config(json!({}), true);
    assert_config(json!({"set": [], "add": [], "remove": []}), true);
    assert_config(
        json!({"set": [{"name": "x".repeat(256), "value": "x".repeat(4096)}]}),
        true,
    );
    assert_config(
        json!({"add": [{"name": "!#$%&'*+-.^_`|~Az09", "value": "é".repeat(4096)}]}),
        true,
    );
    assert_config(
        json!({"set": [{"name": "X-Literal", "value": " ${request:value}\t "}], "remove": ["x".repeat(256)]}),
        true,
    );
    for count in [16, 17] {
        for field in ["set", "add"] {
            let entries: Vec<_> = (0..count)
                .map(|i| json!({"name": format!("x-{i}"), "value": "ok"}))
                .collect();
            assert_config(json!({field: entries}), count == 16);
        }
        let names: Vec<_> = (0..count).map(|i| format!("x-{i}")).collect();
        assert_config(json!({"remove": names}), count == 16);
    }
}

#[test]
fn header_modifier_checks_all_duplicates_and_cross_action_conflicts() {
    for field in ["set", "add"] {
        assert_config(
            json!({field: [{"name": "x-test", "value": "first"}, {"name": "X-Test", "value": "second"}]}),
            true,
        );
        assert_config(
            json!({field: [{"name": "x-test", "value": "first"}, {"name": "X-Test", "value": "bad\nvalue"}]}),
            false,
        );
    }
    assert_config(json!({"remove": ["x-test", "X-Test"]}), true);
    assert_config(
        json!({"set": [{"name": "x-test", "value": "ok"}], "add": [{"name": "X-Test", "value": "other"}]}),
        false,
    );
    for field in ["set", "add"] {
        assert_config(
            json!({field: [{"name": "x-test", "value": "ok"}], "remove": ["X-Test"]}),
            false,
        );
    }
    assert_config(
        json!({"set": [{"name": "x-set", "value": "ok"}], "add": [{"name": "x-add", "value": "ok"}], "remove": ["x-remove"]}),
        true,
    );
}

fn assert_lists(filters: Value, valid: bool) {
    let http: Vec<HTTPRouteFilter> = serde_json::from_value(filters.clone()).unwrap();
    let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(filters.clone()).unwrap();
    assert_eq!(validate_http_filter_list(&http).is_ok(), valid, "filters: {filters}");
    assert_eq!(validate_grpc_filter_list(&grpc), validate_http_filter_list(&http));
    for rule in [
        json!({"filters": filters}),
        json!({"backendRefs": [{"name": "backend", "port": 80, "filters": filters}]}),
    ] {
        let http: HTTPRouteRule = serde_json::from_value(rule.clone()).unwrap();
        let grpc: GRPCRouteRule = serde_json::from_value(rule).unwrap();
        assert_eq!(http.header_modifier_error().is_none(), valid);
        assert_eq!(grpc.header_modifier_error(), http.header_modifier_error());
    }
}

#[test]
fn response_header_policy_is_directional_for_native_rules_and_backends() {
    for name in ["Content-Length", "cOnNeCtIoN", "Proxy-Connection", "TE"] {
        for (kind, field, valid) in [
            ("ResponseHeaderModifier", "responseHeaderModifier", false),
            ("RequestHeaderModifier", "requestHeaderModifier", true),
        ] {
            for action in ["set", "add", "remove"] {
                let value = if action == "remove" {
                    json!([name])
                } else {
                    json!([{"name":name, "value":"1"}])
                };
                assert_lists(json!([{"type":kind, field:{action:value}}]), valid);
            }
        }
    }
}

#[test]
fn header_modifier_native_pairing_and_repetition_cover_rule_and_backend_lists() {
    assert_lists(json!([]), true);
    assert_lists(
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {}}, {"type": "ResponseHeaderModifier", "responseHeaderModifier": {}}]),
        true,
    );
    for (kind, field, other) in [
        (
            "RequestHeaderModifier",
            "requestHeaderModifier",
            "responseHeaderModifier",
        ),
        (
            "ResponseHeaderModifier",
            "responseHeaderModifier",
            "requestHeaderModifier",
        ),
    ] {
        assert_lists(json!([{"type": kind}]), false);
        assert_lists(json!([{"type": kind, other: {}}]), false);
        assert_lists(json!([{"type": kind, field: {}, other: {}}]), false);
        assert_lists(
            json!([{"type": kind, field: {}, "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}}]),
            false,
        );
        assert_lists(json!([{"type": "ExtensionRef", field: {}}]), false);
        assert_lists(json!([{"type": kind, field: {}}, {"type": kind, field: {}}]), false);
        assert_lists(
            json!([{"type": kind, field: {"set": [{"name": "x-test", "value": "invalid\nvalue"}]}}]),
            false,
        );
    }
}

#[test]
fn header_modifier_http_rejects_unrelated_payload_on_header_filter() {
    for (field, value) in [
        ("requestRedirect", json!({})),
        ("urlRewrite", json!({})),
        ("requestMirror", json!({"backendRef": {"name": "mirror", "port": 80}})),
        ("cors", json!({})),
        (
            "externalAuth",
            json!({"target": {"name": "auth", "port": 8080}, "decision": {}}),
        ),
    ] {
        let filters: Vec<HTTPRouteFilter> = serde_json::from_value(
            json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {}, field: value}]),
        )
        .unwrap();
        assert!(validate_http_filter_list(&filters).is_err(), "unrelated field: {field}");
    }
}
