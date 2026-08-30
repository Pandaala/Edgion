//! Drift guard for the EdgionPlugins CRD `type` discriminator enums.
//!
//! The hand-written CRD `config/crd/edgion-crd/edgion_plugins_crd.yaml` declares
//! a closed `type` enum on each of its plugin-stage list fields. Those enums are
//! validated at admission time (controller Admin API and the K8s API server), so
//! a `type` value that is missing from the enum of the stage where the plugin is
//! actually valid is un-appliable, while a stale enum entry that names no real
//! plugin is dead weight (admitted, then fails enum deserialize downstream).
//!
//! `crd_coverage.rs` field-guards only the two fully-structural CRDs; the plugin
//! CRD models `config` as `x-kubernetes-preserve-unknown-fields` and is NOT
//! covered there, which is how `UrlRewrite`/`FaultInjection` silently went
//! missing and `ExtensionRef`/`WebhookKeyGet` lingered as dead entries. This test
//! closes that gap with two checks driven by `schema_for!(EdgionPlugin)` (the
//! variant universe is read from the schema, so adding a variant updates it
//! automatically):
//!
//! * Direction-1 (no un-appliable variant): for EACH stage, every variant valid
//!   at that stage appears in that stage's CRD enum. This is a per-stage SUPERSET
//!   check — checking only "appears in some enum" would be fooled by the lenient
//!   request-stage entries below (a variant could be dropped from its real stage
//!   enum yet still be listed in `requestPlugins`, masking that it is now
//!   un-appliable at its only valid stage).
//! * Direction-2 (no dead/duplicate entry): every stage-enum entry names a real
//!   `EdgionPlugin` variant, and no stage enum repeats a value.
//!
//! Per the project's lenient-admission policy the guard intentionally does NOT
//! require a stage enum to EXCLUDE plugins valid only at another stage (it is a
//! superset, not an equality, check) — those extra entries flow through the
//! `Accepted=False`/Invalid status condition rather than a hard admission
//! rejection. The `upstreamResponsePlugins` field has no `type` enum (free-form);
//! a plugin valid ONLY at that async stage is therefore not required in any
//! enum, which is expected (today `ExtProc` is the sole async plugin and it is
//! also request-valid).
//!
//! The per-stage applicability table below mirrors the exception arms of
//! `applicable_stages` in `edgion_plugins/validate.rs` (the runtime authority,
//! itself an exhaustive match kept in lockstep with the gateway). Only the
//! non-(request-only) variants are listed; every other variant defaults to
//! request-stage-only, exactly as that match does.

use edgion_resources::resources::edgion_plugins::{applicable_stages_by_name, HTTP_PLUGIN_STAGE_EXCEPTIONS};
use schemars::schema_for;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

// Stage identifiers (internal keys, not user-facing strings).
const REQUEST: &str = "request";
const RESPONSE_FILTER: &str = "upstreamResponseFilter";
const RESPONSE_BODY_FILTER: &str = "upstreamResponseBodyFilter";
const RESPONSE_ASYNC: &str = "upstreamResponse";

/// CRD stage list fields that carry a closed `type` enum, paired with the stage
/// key whose valid variants that enum must cover. `upstreamResponsePlugins` is
/// intentionally absent (free-form, no enum).
const STAGE_FIELD_KEYS: &[(&str, &str)] = &[
    ("requestPlugins", REQUEST),
    ("upstreamResponseFilterPlugins", RESPONSE_FILTER),
    ("upstreamResponseBodyFilterPlugins", RESPONSE_BODY_FILTER),
];

/// Stages a variant is valid for, read from the authority in `validate.rs`.
///
/// This guard used to keep its own `STAGE_EXCEPTIONS` mirror of
/// `applicable_stages`. That mirror is gone: `HTTP_PLUGIN_STAGE_EXCEPTIONS` is
/// now a `pub const` in `src/`, and this reads it. A drift guard that has to
/// duplicate the table it guards can drift itself, which is exactly what
/// happened to the equivalent mirror beside `define_resources!`.
fn stages_for(variant: &str) -> BTreeSet<&'static str> {
    let s = applicable_stages_by_name(variant);
    let mut out = BTreeSet::new();
    if s.request {
        out.insert(REQUEST);
    }
    if s.upstream_response_filter {
        out.insert(RESPONSE_FILTER);
    }
    if s.upstream_response_body_filter {
        out.insert(RESPONSE_BODY_FILTER);
    }
    if s.upstream_response {
        out.insert(RESPONSE_ASYNC);
    }
    out
}

/// Authoritative set of plugin `type` discriminator values, read from the
/// `oneOf` branches of `schema_for!(EdgionPlugin)` (each branch carries
/// `properties.type.const = "<VariantName>"`).
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

/// Parse the plugin CRD and return `(field -> ordered list of enum entries)` for
/// every stage field that declares a `type` enum.
fn crd_stage_enums() -> Vec<(&'static str, Vec<String>)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_plugins_crd.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let doc: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let props = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"];
    assert!(
        props.is_object(),
        "plugin CRD spec.properties is not an object; the CRD YAML path may have changed"
    );
    STAGE_FIELD_KEYS
        .iter()
        .map(|(field, _stage)| {
            let enum_node = &props[field]["items"]["properties"]["type"]["enum"];
            let entries = enum_node
                .as_array()
                .unwrap_or_else(|| panic!("CRD field {field}.items.properties.type.enum is not an array"))
                .iter()
                .map(|v| {
                    v.as_str()
                        .unwrap_or_else(|| panic!("CRD field {field} type enum has a non-string entry: {v:?}"))
                        .to_string()
                })
                .collect();
            (*field, entries)
        })
        .collect()
}

/// Restores the compile-time guarantee that the old exhaustive `match` in
/// `applicable_stages` provided.
///
/// The stage table is now a lookup keyed by `type_name()`, so a typo'd key
/// would silently make its plugin request-only instead of failing the build.
/// Validating the keys against `schema_for!(EdgionPlugin)` is strictly stronger
/// than the match was: it checks the names the CRD and `plugin_policy` actually
/// consume, not the enum declaration.
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

/// No plugin type is listed as request-only-and-also-a-response-stage entry.
///
/// A duplicate of a variant's own name across stage enums is legitimate (`Dsl`,
/// `Wasm`), so this asserts the stronger property: every entry in a
/// response-stage enum is genuinely valid at that stage per the authority.
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

/// Direction-3: every entry in a response-stage enum is genuinely valid there.
///
/// Nothing else catches this. Direction-1 is a deliberate SUPERSET check (a
/// stage enum may carry extra entries), and Direction-2 only rejects names that
/// match no variant at all — so a request-only plugin wrongly listed in
/// `upstreamResponseFilterPlugins` would slip past both, and an operator could
/// apply it at a stage where it silently never runs.
///
/// Only the two response-stage enums are checked. `requestPlugins` is
/// deliberately lenient (it lists response-stage plugins too — see the
/// module-level note on lenient admission), so an entry there implies nothing
/// about the variant's real stages.
///
/// This asserts against `applicable_stages_by_name`, the same authority
/// Direction-1 reads. Before the stage table was single-sourced it could only
/// assert the weaker "is declared in the mirror table".
#[test]
fn response_stage_enum_entries_are_valid_at_that_stage() {
    let stage_enums = crd_stage_enums();

    let mut undeclared: Vec<String> = Vec::new();
    for (field, stage) in STAGE_FIELD_KEYS {
        if *stage == REQUEST {
            continue;
        }
        let entries: &[String] = stage_enums
            .iter()
            .find(|(f, _)| f == field)
            .map(|(_, e)| e.as_slice())
            .unwrap_or(&[]);
        for e in entries {
            if !stages_for(e).contains(stage) {
                undeclared.push(format!("{field}: {e}"));
            }
        }
    }
    undeclared.sort();
    assert!(
        undeclared.is_empty(),
        "CRD lists plugin type(s) in a non-request stage enum that are NOT valid at that \
         stage:\n  {undeclared:?}\n\
         Fix one of two ways: if the plugin really does run at that stage, add it to \
         HTTP_PLUGIN_STAGE_EXCEPTIONS in edgion_plugins/validate.rs (and wire the matching \
         gateway constructor); otherwise remove the entry from edgion_plugins_crd.yaml — an \
         operator could apply it there today and it would silently never run."
    );
}

#[test]
fn every_stage_valid_variant_is_in_that_stage_enum() {
    let variants = rust_variant_type_names();
    let stage_enums = crd_stage_enums();

    let mut missing: Vec<String> = Vec::new();
    for (field, stage) in STAGE_FIELD_KEYS {
        let crd_set: BTreeSet<&str> = stage_enums
            .iter()
            .find(|(f, _)| f == field)
            .map(|(_, e)| e.iter().map(String::as_str).collect())
            .unwrap_or_default();
        for v in &variants {
            if stages_for(v).contains(stage) && !crd_set.contains(v.as_str()) {
                missing.push(format!("{field}: {v}"));
            }
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "EdgionPlugin variant(s) valid at a stage but absent from that stage's CRD `type` enum \
         — they cannot be applied there:\n  {missing:?}\n\
         Fix: add each to the named stage enum in edgion_plugins_crd.yaml (and, if a new variant \
         changed stages, update STAGE_EXCEPTIONS to mirror applicable_stages)."
    );
}

#[test]
fn no_stage_enum_has_dead_or_duplicate_entries() {
    let variants = rust_variant_type_names();
    let stage_enums = crd_stage_enums();

    let mut dead: Vec<String> = Vec::new();
    let mut dups: Vec<String> = Vec::new();
    for (field, entries) in &stage_enums {
        let mut seen = BTreeSet::new();
        for e in entries {
            if !variants.contains(e) {
                dead.push(format!("{field}: {e}"));
            }
            if !seen.insert(e.clone()) {
                dups.push(format!("{field}: {e}"));
            }
        }
    }
    assert!(
        dead.is_empty() && dups.is_empty(),
        "EdgionPlugins CRD `type` enum drift:\n  dead entries (no matching EdgionPlugin variant): {dead:?}\n  \
         duplicate entries: {dups:?}\n\
         Fix: remove the stale/duplicate value(s) from edgion_plugins_crd.yaml, \
         or rename them to a real EdgionPlugin variant."
    );
}

#[test]
fn logical_waf_schema_is_top_level_strict_and_not_a_stage_discriminator() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_plugins_crd.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let doc: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let props = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"];
    let waf = &props["waf"];

    assert_eq!(waf["type"], "object");
    assert_eq!(waf["additionalProperties"], false);
    assert_eq!(waf["required"], serde_json::json!(["rules"]));
    assert_eq!(
        waf["properties"]["mode"]["enum"],
        serde_json::json!(["on", "detectionOnly"])
    );
    assert!(waf["properties"]["rules"]["minLength"]
        .as_u64()
        .is_some_and(|value| value > 0));

    for (field, _) in STAGE_FIELD_KEYS {
        let entries = props[field]["items"]["properties"]["type"]["enum"]
            .as_array()
            .expect("stage discriminator is an array");
        assert!(!entries.iter().any(|entry| entry == "Waf"));
    }
}
