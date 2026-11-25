//! Open plugin discriminator contract and schema-driven stage metadata checks.
use edgion_resources::resources::edgion_plugins::HTTP_PLUGIN_STAGE_EXCEPTIONS;
use schemars::schema_for;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn rust_variant_type_names() -> BTreeSet<String> {
    let schema = serde_json::to_value(schema_for!(edgion_resources::resources::edgion_plugins::EdgionPlugin)).unwrap();
    let branches = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .expect("EdgionPlugin schema must expose a oneOf of tagged variants");
    let names: BTreeSet<String> = branches
        .iter()
        .filter_map(|b| {
            b.get("properties")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.get("const"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        names.len() >= 30,
        "extracted only {} variant type names from the EdgionPlugin schema; the \
         schemars output shape likely changed and this guard is no longer reading it",
        names.len()
    );
    names
}

#[test]
fn stage_exception_keys_are_real_variants() {
    let variants = rust_variant_type_names();
    let bad: Vec<&str> = HTTP_PLUGIN_STAGE_EXCEPTIONS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !variants.contains(*name))
        .collect();
    assert!(
        bad.is_empty(),
        "HTTP_PLUGIN_STAGE_EXCEPTIONS names a non-existent EdgionPlugin variant: {bad:?} — \
         fix the typo in edgion_plugins/validate.rs; the key must equal the variant's `type_name()`."
    );
}

#[test]
fn stage_exception_table_has_no_request_only_entries() {
    let bogus: Vec<&str> = HTTP_PLUGIN_STAGE_EXCEPTIONS
        .iter()
        .filter(|(_, s)| !s.upstream_response_filter && !s.upstream_response_body_filter && !s.upstream_response)
        .map(|(name, _)| *name)
        .collect();
    assert!(
        bogus.is_empty(),
        "HTTP_PLUGIN_STAGE_EXCEPTIONS lists request-only plugin(s) {bogus:?} — the table is for \
         NON-request-only plugins; request-only is the default and needs no entry."
    );
}

#[test]
fn plugin_crd_types_are_open_strings_in_every_stage() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_plugins_crd.yaml");
    let doc: Value = yaml_serde::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let props = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"];
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        let item = &props[stage]["items"];
        let mut discriminator = item["properties"]["type"].clone();
        discriminator.as_object_mut().unwrap().remove("description");
        assert_eq!(
            discriminator,
            serde_json::json!({"type":"string"}),
            "{stage}: type must remain open"
        );
        assert_eq!(
            item["properties"]["config"]["x-kubernetes-preserve-unknown-fields"],
            true
        );
    }
}

#[test]
fn unsupported_sentinel_is_not_a_supported_plugin_or_policy_name() {
    assert!(!rust_variant_type_names().contains("Unsupported"));
    assert!(!rust_variant_type_names().contains("InvalidConfig"));
    use edgion_resources::resources::plugin_policy::{canonical_type_names, PluginFamily};
    for family in [PluginFamily::Http, PluginFamily::Stream, PluginFamily::TlsRoute] {
        assert!(!canonical_type_names(family).contains("Unsupported"));
        assert!(!canonical_type_names(family).contains("InvalidConfig"));
    }
}
