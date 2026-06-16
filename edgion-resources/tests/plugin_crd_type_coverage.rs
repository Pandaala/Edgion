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

/// Non-(request-only) variants and the stages they are valid for. Mirrors the
/// exception arms of `applicable_stages` (validate.rs); every variant NOT listed
/// here is request-stage-only. Keep this in lockstep with that match — a
/// misspelled key is caught by `stage_exception_keys_are_real_variants`.
const STAGE_EXCEPTIONS: &[(&str, &[&str])] = &[
    ("ResponseHeaderModifier", &[RESPONSE_FILTER]),
    ("DebugAccessLogToHeader", &[RESPONSE_FILTER]),
    ("ResponseRewrite", &[RESPONSE_FILTER]),
    ("BandwidthLimit", &[RESPONSE_BODY_FILTER]),
    ("Dsl", &[REQUEST, RESPONSE_FILTER]),
    ("ExtProc", &[REQUEST, RESPONSE_ASYNC]),
];

/// Stages a variant is valid for: an explicit exception, else request-only.
fn stages_for(variant: &str) -> BTreeSet<&'static str> {
    for (name, stages) in STAGE_EXCEPTIONS {
        if *name == variant {
            return stages.iter().copied().collect();
        }
    }
    [REQUEST].into_iter().collect()
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

#[test]
fn stage_exception_keys_are_real_variants() {
    let variants = rust_variant_type_names();
    let bad: Vec<&str> = STAGE_EXCEPTIONS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !variants.contains(*name))
        .collect();
    assert!(
        bad.is_empty(),
        "STAGE_EXCEPTIONS names a non-existent EdgionPlugin variant: {bad:?} — \
         fix the typo or update the table to match `applicable_stages` in validate.rs."
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
