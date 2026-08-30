//! Drift guard for the EdgionStreamPlugins CRD `type` discriminator enums.
//!
//! The stream analog of `plugin_crd_type_coverage.rs`. `edgion_stream_plugins_crd.yaml`
//! is hand-written and, like the HTTP plugin CRD, models `config` as
//! `x-kubernetes-preserve-unknown-fields`, so `crd_coverage.rs` does NOT field-guard
//! it — it only pins that `tlsRoutePlugins` exists as a property. The one thing the
//! CRD still constrains is the `type:` discriminator, and until now nothing checked
//! it against the Rust enums.
//!
//! Unlike the HTTP guard there is no stage-applicability table to mirror: each CRD
//! field maps 1:1 onto exactly one Rust enum, so this is a plain set equality in
//! both directions:
//!
//! * Direction-1 (no un-appliable variant): every Rust variant appears in its
//!   field's CRD enum. A missing entry means the plugin cannot be applied at all.
//! * Direction-2 (no dead/duplicate entry): every CRD enum entry names a real
//!   variant, and no enum repeats a value. A stale entry is admitted by the API
//!   server and then fails enum deserialize downstream.
//!
//! The variant universe is read from `schema_for!`, so adding a variant updates
//! the expected set automatically — there is no list to maintain here.

use schemars::schema_for;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// CRD `spec.properties` field names. These are the serde names of
/// `EdgionStreamPluginsSpec::{plugins, tls_route_plugins}` — note the Stage-1
/// field is `plugins`, not `streamPlugins`.
const STREAM_PLUGINS_FIELD: &str = "plugins";
const TLS_ROUTE_PLUGINS_FIELD: &str = "tlsRoutePlugins";

/// Pull the `type` discriminator values out of a tagged-enum schema's `oneOf`
/// branches (each branch carries `properties.type.const = "<VariantName>"`).
fn variant_type_names(schema: Value, what: &str) -> BTreeSet<String> {
    let branches = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{what} schema must expose a oneOf of tagged variants"));
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
        !names.is_empty(),
        "extracted zero variant type names from the {what} schema; the schemars output \
         shape likely changed and this guard is no longer reading it"
    );
    names
}

fn stream_plugin_variants() -> BTreeSet<String> {
    variant_type_names(
        serde_json::to_value(schema_for!(
            edgion_resources::resources::edgion_stream_plugins::EdgionStreamPlugin
        ))
        .unwrap(),
        "EdgionStreamPlugin",
    )
}

fn tls_route_plugin_variants() -> BTreeSet<String> {
    variant_type_names(
        serde_json::to_value(schema_for!(
            edgion_resources::resources::edgion_stream_plugins::TlsRouteStreamPlugin
        ))
        .unwrap(),
        "TlsRouteStreamPlugin",
    )
}

/// Ordered `type` enum entries for one CRD stage field (duplicates preserved, so
/// Direction-2 can detect them).
fn crd_type_enum(field: &str) -> Vec<String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_stream_plugins_crd.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let doc: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let props = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"];
    assert!(
        props.is_object(),
        "stream plugin CRD spec.properties is not an object; the CRD YAML path may have changed"
    );
    props[field]["items"]["properties"]["type"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("CRD field {field}.items.properties.type.enum is not an array"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("CRD field {field} type enum has a non-string entry: {v:?}"))
                .to_string()
        })
        .collect()
}

fn assert_parity(field: &str, rust_variants: &BTreeSet<String>, enum_name: &str) {
    let entries = crd_type_enum(field);

    let mut seen = BTreeSet::new();
    let dups: Vec<&String> = entries.iter().filter(|e| !seen.insert((*e).clone())).collect();
    assert!(
        dups.is_empty(),
        "{field}: duplicate CRD `type` enum entries {dups:?} — remove the repeats from \
         edgion_stream_plugins_crd.yaml."
    );

    let crd_set: BTreeSet<String> = entries.into_iter().collect();

    let missing: Vec<&String> = rust_variants.difference(&crd_set).collect();
    assert!(
        missing.is_empty(),
        "{field}: `{enum_name}` variant(s) absent from the CRD `type` enum, so they cannot be \
         applied: {missing:?}\nFix: add each to the {field} type enum in \
         edgion_stream_plugins_crd.yaml."
    );

    let dead: Vec<&String> = crd_set.difference(rust_variants).collect();
    assert!(
        dead.is_empty(),
        "{field}: CRD `type` enum entr(ies) naming no real `{enum_name}` variant: {dead:?}\n\
         Fix: remove the stale value(s) from edgion_stream_plugins_crd.yaml, or rename them to a \
         real variant."
    );
}

#[test]
fn stream_plugins_crd_type_enum_matches_rust_enum() {
    assert_parity(STREAM_PLUGINS_FIELD, &stream_plugin_variants(), "EdgionStreamPlugin");
}

#[test]
fn tls_route_plugins_crd_type_enum_matches_rust_enum() {
    assert_parity(
        TLS_ROUTE_PLUGINS_FIELD,
        &tls_route_plugin_variants(),
        "TlsRouteStreamPlugin",
    );
}
