use super::*;
use serde_json::json;

fn ep(ns: &str, name: &str, spec: serde_json::Value) -> serde_json::Value {
    json!({
        "metadata": { "namespace": ns, "name": name },
        "spec": spec,
    })
}

#[test]
fn no_nested_ext_ref_returns_none() {
    let v = ep(
        "default",
        "a",
        json!({
            "requestPlugins": [
                { "type": "BasicAuth", "config": { "users": [] } }
            ]
        }),
    );
    assert!(first_nested_ext_ref_stage(&v).is_none());
}

#[test]
fn nested_ext_ref_in_request_detected() {
    let v = ep(
        "default",
        "a",
        json!({
            "requestPlugins": [
                { "type": "ExtensionRef", "config": { "name": "B" } }
            ]
        }),
    );
    assert_eq!(first_nested_ext_ref_stage(&v), Some("request"));
}

#[test]
fn nested_ext_ref_in_body_filter_detected() {
    let v = ep(
        "default",
        "a",
        json!({
            "upstreamResponseBodyFilterPlugins": [
                { "type": "ExtensionRef", "config": { "name": "B" } }
            ]
        }),
    );
    assert_eq!(first_nested_ext_ref_stage(&v), Some("upstreamResponseBodyFilter"));
}

#[test]
fn first_stage_wins_when_multiple_present() {
    let v = ep(
        "default",
        "a",
        json!({
            "requestPlugins": [
                { "type": "ExtensionRef", "config": { "name": "B" } }
            ],
            "upstreamResponsePlugins": [
                { "type": "ExtensionRef", "config": { "name": "C" } }
            ]
        }),
    );
    assert_eq!(first_nested_ext_ref_stage(&v), Some("request"));
}

#[test]
fn missing_spec_is_safe() {
    let v = json!({ "metadata": { "namespace": "default", "name": "a" } });
    assert!(first_nested_ext_ref_stage(&v).is_none());
}

#[test]
fn extension_ref_at_other_levels_ignored() {
    // A route-level filter (HTTPRoute.rules[].filters) would carry
    // `type: ExtensionRef`, but lives in a different resource entirely;
    // we only scan `EdgionPlugins.spec.*Plugins` so it cannot show up
    // here. Sanity check: an unrelated nested object containing
    // `type: ExtensionRef` outside the scanned arrays is ignored.
    let v = ep(
        "default",
        "a",
        json!({
            "someOtherField": [
                { "type": "ExtensionRef" }
            ],
            "requestPlugins": [
                { "type": "BasicAuth", "config": {} }
            ]
        }),
    );
    assert!(first_nested_ext_ref_stage(&v).is_none());
}

#[test]
fn extract_identity_handles_missing_metadata() {
    let v = json!({});
    let (ns, name) = extract_identity(&v);
    assert_eq!(ns, "");
    assert_eq!(name, "");
}
