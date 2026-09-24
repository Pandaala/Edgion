use super::*;
use crate::resources::edgion_plugins::EdgionPlugin;
use serde_json::json;

fn rule(value: Value) -> HTTPRouteRule {
    serde_json::from_value(value).unwrap()
}

#[test]
fn authored_contract_rejects_pairing_repetition_and_conflicts() {
    let invalid = rule(json!({
        "filters": [
            {"type": "RequestRedirect"},
            {"type": "RequestRedirect", "requestRedirect": {}},
            {"type": "URLRewrite", "urlRewrite": {}}
        ]
    }));
    let violations = authored_request_transform_violations(&invalid).unwrap();
    assert!(violations
        .iter()
        .any(|violation| violation.message.contains("must be specified")));
    assert!(violations
        .iter()
        .any(|violation| violation.message.contains("cannot be repeated")));
    assert!(violations
        .iter()
        .any(|violation| violation.message.contains("not both")));
}

#[test]
fn authored_contract_enforces_the_generated_filter_limit() {
    let filters = (0..17)
        .map(|_| json!({"type": "RequestHeaderModifier", "requestHeaderModifier": {}}))
        .collect::<Vec<_>>();
    let violations = authored_request_transform_violations(&rule(json!({"filters": filters}))).unwrap();
    assert!(violations.iter().any(|violation| violation.path == "filters"));
}

#[test]
fn unrelated_large_match_data_is_outside_the_transform_budget() {
    let headers = (0..16)
        .map(|index| {
            json!({
                "name": format!("x-large-{index}"),
                "value": "x".repeat(4096)
            })
        })
        .collect::<Vec<_>>();
    let matches = (0..5)
        .map(|index| {
            json!({
                "path": {"type": "Exact", "value": format!("/item-{index}")},
                "headers": headers
            })
        })
        .collect::<Vec<_>>();
    let rule = rule(json!({"matches": matches}));

    assert!(authored_request_transform_violations(&rule).unwrap().is_empty());
    assert!(effective_request_transform_violations(&rule).unwrap().is_empty());
}

#[test]
fn authored_contract_uses_match_defaults_for_prefix_rewrite() {
    let valid = rule(json!({
        "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
            "type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"
        }}}]
    }));
    assert!(authored_request_transform_violations(&valid).unwrap().is_empty());

    let invalid = rule(json!({
        "matches": [{"path": {"type": "Exact", "value": "/api"}}],
        "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
            "type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"
        }}}]
    }));
    assert!(!authored_request_transform_violations(&invalid).unwrap().is_empty());
}

#[test]
fn authored_and_effective_validation_include_transport_preparation() {
    let invalid = rule(json!({
        "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
            "type": "ReplaceFullPath", "replaceFullPath": "relative"
        }}}]
    }));
    for violations in [
        authored_request_transform_violations(&invalid).unwrap(),
        effective_request_transform_violations(&invalid).unwrap(),
    ] {
        assert!(violations.iter().any(|violation| {
            violation.path == "filters/0/urlRewrite/path/replaceFullPath"
                && violation.message.contains("absolute HTTP path")
        }));
    }
}

#[test]
fn prepared_redirect_is_typed_and_transport_safe() {
    let config: HTTPRequestRedirectFilter = serde_json::from_value(json!({
        "scheme": "https", "port": 8443, "statusCode": 307,
        "path": {"type": "ReplaceFullPath", "replaceFullPath": "/login"}
    }))
    .unwrap();
    let prepared = config.prepare().unwrap();
    assert_eq!(prepared.scheme(), Some(PreparedRedirectScheme::Https));
    assert_eq!(prepared.port().map(NonZeroU16::get), Some(8443));
    assert_eq!(prepared.status_code(), StatusCode::TEMPORARY_REDIRECT);

    let invalid: HTTPURLRewriteFilter = serde_json::from_value(json!({
        "path": {"type": "ReplaceFullPath", "replaceFullPath": "/x?leak=1"}
    }))
    .unwrap();
    assert!(invalid.prepare().is_err());

    for replacement in ["relative", "/fragment#part"] {
        let invalid: HTTPURLRewriteFilter = serde_json::from_value(json!({
            "path": {"type": "ReplaceFullPath", "replaceFullPath": replacement}
        }))
        .unwrap();
        assert!(invalid.prepare().is_err());
    }
}

#[test]
fn empty_full_path_normalizes_and_empty_prefix_is_valid() {
    let full: HTTPURLRewriteFilter = serde_json::from_value(json!({
        "path": {"type": "ReplaceFullPath", "replaceFullPath": ""}
    }))
    .unwrap();
    assert_eq!(
        full.prepare().unwrap().path(),
        Some(&PreparedPathModifier::ReplaceFullPath("/".to_string()))
    );

    let prefix: HTTPURLRewriteFilter = serde_json::from_value(json!({
        "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": ""}
    }))
    .unwrap();
    assert_eq!(
        prefix.prepare().unwrap().path(),
        Some(&PreparedPathModifier::ReplacePrefixMatch(String::new()))
    );
}

#[test]
fn path_modifier_nulls_remain_invalid() {
    assert!(serde_json::from_value::<HTTPURLRewriteFilter>(json!({
        "path": {"type": null, "replaceFullPath": "/v2"}
    }))
    .is_err());

    let missing_replacement: HTTPURLRewriteFilter = serde_json::from_value(json!({
        "path": {"type": "ReplaceFullPath", "replaceFullPath": null}
    }))
    .unwrap();
    assert!(missing_replacement.prepare().is_err());
}

#[test]
fn typed_path_match_defaults_come_from_the_contract() {
    let explicit_empty: HTTPRouteMatch = serde_json::from_value(json!({"path": {}})).unwrap();
    let path = explicit_empty.path.unwrap();
    assert_eq!(path.match_type.as_deref(), Some("PathPrefix"));
    assert_eq!(path.value.as_deref(), Some("/"));

    let omitted: HTTPRouteMatch = serde_json::from_value(json!({})).unwrap();
    assert_eq!(omitted.path.unwrap().value.as_deref(), Some("/"));

    for (input, expected_type, expected_value) in [
        (json!({"path": null}), "PathPrefix", "/"),
        (json!({"path": {"type": null, "value": "/api"}}), "PathPrefix", "/api"),
        (json!({"path": {"type": "Exact", "value": null}}), "Exact", "/"),
    ] {
        let json_match: HTTPRouteMatch = serde_json::from_value(input.clone()).unwrap();
        let yaml_match: HTTPRouteMatch = serde_yaml::from_str(&serde_yaml::to_string(&input).unwrap()).unwrap();
        for parsed in [json_match, yaml_match] {
            let path = parsed.path.as_ref().unwrap();
            assert_eq!(path.match_type.as_deref(), Some(expected_type));
            assert_eq!(path.value.as_deref(), Some(expected_value));

            let encoded = serde_json::to_value(&parsed).unwrap();
            let round_trip: HTTPRouteMatch = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(serde_json::to_value(round_trip).unwrap(), encoded);
        }
    }
}

#[test]
fn edgion_plugin_uses_the_same_config_fragment() {
    let plugin = EdgionPlugin::RequestRedirect(serde_json::from_value(json!({"scheme": "ftp"})).unwrap());
    assert!(plugin.validation_error().is_some());
}

#[test]
fn effective_safety_catches_multiple_backend_prefix_transforms() {
    let rule = rule(json!({
        "matches": [{"path": {"type": "Exact", "value": "/api"}}],
        "backendRefs": [
            {"name": "a", "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                "type": "ReplacePrefixMatch", "replacePrefixMatch": "/a"
            }}}]},
            {"name": "b", "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                "type": "ReplacePrefixMatch", "replacePrefixMatch": "/b"
            }}}]}
        ]
    }));
    for violations in [
        authored_request_transform_violations(&rule).unwrap(),
        effective_request_transform_violations(&rule).unwrap(),
    ] {
        assert!(violations
            .iter()
            .any(|violation| violation.message.contains("PathPrefix")));
    }
}
