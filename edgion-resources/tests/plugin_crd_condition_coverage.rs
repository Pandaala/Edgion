//! Drift guard for the EdgionPlugins CRD bounded ConditionSet subtree.
//!
//! The hand-written CRD models a condition as one flat merged property bag —
//! `type`, `key`, `values`, `regex`, `caseSensitive`, `negate`, `after`,
//! `before`, `ratio` as siblings of a single object, no `oneOf`.
//! `schema_for!(Condition)` (schemars 1.x) produces a discriminated `oneOf`
//! of variant objects:
//!
//! * Top-level keys: `$defs`, `$schema`, `description`, `oneOf`, `title`.
//! * Each `oneOf` branch: `properties.type.const = "<camelCase>"`, plus a
//!   `"$ref": "#/$defs/<VariantStruct>"` (sometimes via `allOf`). Operator-
//!   facing fields live on the def, not on the branch. Example: the `keyExist`
//!   branch only shows `type`; `key` is on `$defs.KeyExistCondition`.
//!
//! Flatten: for every `oneOf` branch, collect `type.const`; follow `$ref` into
//! `$defs` and union that object's `properties` keys. Compare:
//!
//! * type-enum set vs CRD `properties.type.enum`
//! * unioned leaf-field set vs CRD `properties` keys minus `type`
//!
//! Flattening loses per-variant scoping. That is acceptable for atomic leaves:
//! the CRD uses one closed merged bag for each atomic Condition. Container
//! guards below separately pin the fixed-depth `allOf` / `anyOf` grammar.
//!
//! `#[schemars(skip)]` runtime fields (`values_set`, `compiled_regex`) are
//! already absent from the schema; they are not special-cased here.
//!
//! Public type path: `edgion_resources::resources::edgion_plugins::conditions::Condition`.
//! Not `edgion_resources::resources::common::Condition` (the K8s status struct).
//! `edgion_resources::KeyGet` is the accessor.
//!
//! ## KeyGet
//!
//! `crd_coverage.rs::plugin_key_get_enum_uses_remote_ip_names` is a hard-coded
//! snapshot of the CRD enum only: adding a Rust variant without a CRD entry
//! still passes. This guard reads `schema_for!(KeyGet)` and compares the
//! type-enum both directions against the top-level `allOf` atomic leaf.
//!
//! Nested KeyGet property names (`name`) are not compared. Walking nested
//! KeyGet leaves is out of scope for this type-enum guard.

use edgion_resources::resources::edgion_plugins::conditions::Condition;
use edgion_resources::KeyGet;
use schemars::schema_for;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn plugins_crd_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_plugins_crd.yaml")
}

fn plugins_crd_text() -> String {
    let path = plugins_crd_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn plugins_crd_doc() -> Value {
    let path = plugins_crd_path();
    let text = plugins_crd_text();
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

/// Top-level `allOf` atomic leaf after YAML-anchor expansion:
/// `spec.versions[0].schema.openAPIV3Schema.properties.spec.properties
///  .requestPlugins.items.properties.conditions.properties.skip.properties
///  .allOf.items.properties`
fn crd_condition_properties() -> Map<String, Value> {
    let doc = plugins_crd_doc();
    doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["requestPlugins"]
        ["items"]["properties"]["conditions"]["properties"]["skip"]["properties"]["allOf"]["items"]["properties"]
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "plugin CRD skip.allOf.items.properties is not an object; \
                 the CRD YAML path may have changed"
            )
        })
        .clone()
}

fn crd_conditions_node() -> Value {
    let doc = plugins_crd_doc();
    doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["requestPlugins"]
        ["items"]["properties"]["conditions"]
        .clone()
}

fn crd_any_of_clause_properties() -> Map<String, Value> {
    crd_conditions_node()["properties"]["skip"]["properties"]["anyOf"]["items"]["properties"]
        .as_object()
        .expect("plugin CRD skip.anyOf.items.properties must be an object")
        .clone()
}

fn crd_dye_rule_properties() -> Map<String, Value> {
    let doc = plugins_crd_doc();
    doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["requestPlugins"]
        ["items"]["properties"]["dye"]["properties"]["response"]["items"]["properties"]
        .as_object()
        .expect("plugin CRD dye.response.items.properties must be an object")
        .clone()
}

fn follow_def<'a>(r: &str, defs: &'a Map<String, Value>) -> Option<&'a Value> {
    let name = r.strip_prefix("#/$defs/")?;
    defs.get(name)
}

fn branch_type_const(branch: &Value) -> Option<String> {
    branch
        .get("properties")
        .and_then(|p| p.get("type"))
        .and_then(|t| t.get("const"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn branch_refs(branch: &Value) -> Vec<&str> {
    let mut refs = Vec::new();
    if let Some(r) = branch.get("$ref").and_then(Value::as_str) {
        refs.push(r);
    }
    if let Some(all_of) = branch.get("allOf").and_then(Value::as_array) {
        for item in all_of {
            if let Some(r) = item.get("$ref").and_then(Value::as_str) {
                refs.push(r);
            }
        }
    }
    refs
}

/// Flatten a tagged-union schemars schema into `(type-const set, leaf-field set)`.
///
/// One hop only: `oneOf` branch → `$ref` def → that object's `properties` keys.
/// Nested objects (including `KeyGet` under `key`) are not walked.
fn flatten_tagged_union(schema: &Value) -> (BTreeSet<String>, BTreeSet<String>) {
    let defs = schema.get("$defs").and_then(Value::as_object);
    let branches = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .expect("tagged-union schema must expose a oneOf of variant objects");
    let mut types = BTreeSet::new();
    let mut fields = BTreeSet::new();
    for branch in branches {
        let type_name = branch_type_const(branch).unwrap_or_else(|| {
            panic!("oneOf branch is missing properties.type.const: {branch}");
        });
        types.insert(type_name);
        if let Some(props) = branch.get("properties").and_then(Value::as_object) {
            for key in props.keys() {
                if key != "type" {
                    fields.insert(key.clone());
                }
            }
        }
        if let Some(defs) = defs {
            for r in branch_refs(branch) {
                let target = follow_def(r, defs).unwrap_or_else(|| panic!("$ref {r} is not in $defs"));
                if let Some(props) = target.get("properties").and_then(Value::as_object) {
                    for key in props.keys() {
                        if key != "type" {
                            fields.insert(key.clone());
                        }
                    }
                }
            }
        }
    }
    (types, fields)
}

fn rust_condition_sets() -> (BTreeSet<String>, BTreeSet<String>) {
    let schema = serde_json::to_value(schema_for!(Condition)).unwrap();
    assert!(
        schema.get("oneOf").is_some() && schema.get("$defs").is_some(),
        "Condition schema top-level shape changed (expected oneOf + $defs): keys={:?}",
        schema.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>())
    );
    let (types, fields) = flatten_tagged_union(&schema);
    assert!(
        types.len() >= 10,
        "extracted only {} Condition type consts; the schemars output shape \
         likely changed and this guard is no longer reading it",
        types.len()
    );
    (types, fields)
}

fn rust_key_get_types() -> BTreeSet<String> {
    let schema = serde_json::to_value(schema_for!(KeyGet)).unwrap();
    let (types, _fields) = flatten_tagged_union(&schema);
    assert!(
        types.len() >= 9,
        "extracted only {} KeyGet type consts; the schemars output shape \
         likely changed and this guard is no longer reading it",
        types.len()
    );
    types
}

fn crd_enum_values(node: &Value, label: &str) -> BTreeSet<String> {
    node.as_array()
        .unwrap_or_else(|| panic!("{label} is not an array"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("{label} has a non-string entry: {v:?}"))
                .to_string()
        })
        .collect()
}

fn crd_condition_sets() -> (BTreeSet<String>, BTreeSet<String>) {
    let props = crd_condition_properties();
    let types = crd_enum_values(
        props.get("type").and_then(|t| t.get("enum")).unwrap_or(&Value::Null),
        "CRD condition properties.type.enum",
    );
    let fields: BTreeSet<String> = props.keys().filter(|k| *k != "type").cloned().collect();
    (types, fields)
}

fn crd_key_get_types() -> BTreeSet<String> {
    let props = crd_condition_properties();
    crd_enum_values(
        &props["key"]["properties"]["type"]["enum"],
        "CRD conditions.skip.allOf.items.properties.key.properties.type.enum",
    )
}

/// Shared two-direction set diff. Direction-1 = rust-only (missing from CRD);
/// Direction-2 = CRD-only (unbacked).
fn diff_sets(rust: &BTreeSet<String>, crd: &BTreeSet<String>) -> (BTreeSet<String>, BTreeSet<String>) {
    (
        rust.difference(crd).cloned().collect(),
        crd.difference(rust).cloned().collect(),
    )
}

/// Compare flattened Condition type-enum + leaf-field sets both directions.
/// Tests 1 / 4 / 5 must all call this function; do not re-implement the diff.
fn compare_condition_coverage(
    rust_types: &BTreeSet<String>,
    rust_fields: &BTreeSet<String>,
    crd_types: &BTreeSet<String>,
    crd_fields: &BTreeSet<String>,
) -> ConditionCoverage {
    let (d1_types, d2_types) = diff_sets(rust_types, crd_types);
    let (d1_fields, d2_fields) = diff_sets(rust_fields, crd_fields);
    ConditionCoverage {
        d1_types,
        d2_types,
        d1_fields,
        d2_fields,
    }
}

struct ConditionCoverage {
    d1_types: BTreeSet<String>,
    d2_types: BTreeSet<String>,
    d1_fields: BTreeSet<String>,
    d2_fields: BTreeSet<String>,
}

impl ConditionCoverage {
    fn is_clean(&self) -> bool {
        self.d1_types.is_empty() && self.d2_types.is_empty() && self.d1_fields.is_empty() && self.d2_fields.is_empty()
    }

    fn report(&self) -> String {
        format!(
            "Condition CRD drift:\n  \
             Direction-1 missing from CRD (types): {:?}\n  \
             Direction-1 missing from CRD (fields): {:?}\n  \
             Direction-2 stale in CRD (types): {:?}\n  \
             Direction-2 stale in CRD (fields): {:?}",
            self.d1_types, self.d1_fields, self.d2_types, self.d2_fields
        )
    }
}

fn compare_key_get_types(rust: &BTreeSet<String>, crd: &BTreeSet<String>) -> (BTreeSet<String>, BTreeSet<String>) {
    diff_sets(rust, crd)
}

#[test]
fn condition_schema_matches_crd_both_directions() {
    let (rust_types, rust_fields) = rust_condition_sets();
    let (crd_types, crd_fields) = crd_condition_sets();
    let cov = compare_condition_coverage(&rust_types, &rust_fields, &crd_types, &crd_fields);
    assert!(
        cov.is_clean(),
        "{}\n\
         Fix: add the missing identifier(s) to edgion_plugins_crd.yaml `&conditionsSchema`, \
         or remove the stale CRD entry / unused struct field.",
        cov.report()
    );
}

#[test]
fn dye_rule_uses_literal_on_field_name() {
    let properties = crd_dye_rule_properties();
    assert!(
        properties.contains_key("on"),
        "dye rule schema must declare the literal `on` field"
    );
    assert!(
        !properties.contains_key("true"),
        "unquoted YAML 1.1 `on` was coerced into the boolean key `true`"
    );
}

#[test]
fn key_get_type_enum_matches_crd_both_directions() {
    let rust = rust_key_get_types();
    let crd = crd_key_get_types();
    let (d1, d2) = compare_key_get_types(&rust, &crd);
    assert!(
        d1.is_empty() && d2.is_empty(),
        "KeyGet type-enum CRD drift:\n  \
         Direction-1 missing from CRD: {d1:?}\n  \
         Direction-2 stale in CRD: {d2:?}\n\
         Fix: add the missing value(s) to conditions.skip.allOf.items.properties.key.properties.type.enum \
         in edgion_plugins_crd.yaml, or remove the stale CRD entry / unused KeyGet variant."
    );
    assert_eq!(
        rust.len(),
        12,
        "expected twelve KeyGet type consts, got {}: {rust:?}",
        rust.len()
    );
}

fn required_set(node: &Value, label: &str) -> BTreeSet<String> {
    node.get("required")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{label}.required must be an array"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("{label}.required contains a non-string: {value}"))
                .to_string()
        })
        .collect()
}

#[test]
fn condition_set_crd_is_fixed_depth_and_structural() {
    let conditions = crd_conditions_node();
    assert_eq!(conditions["type"], "object");
    assert!(conditions.get("x-kubernetes-preserve-unknown-fields").is_none());

    let skip = &conditions["properties"]["skip"];
    let run = &conditions["properties"]["run"];
    assert_eq!(skip, run, "skip and run must reuse the same ConditionSet schema");
    assert_eq!(skip["type"], "object");
    assert!(skip.get("x-kubernetes-preserve-unknown-fields").is_none());
    let set_fields: BTreeSet<String> = skip["properties"]
        .as_object()
        .expect("ConditionSet properties must be an object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(set_fields, BTreeSet::from(["allOf".to_string(), "anyOf".to_string()]));
    assert!(skip.get("x-kubernetes-validations").is_none());
    assert_eq!(skip["oneOf"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        required_set(&skip["oneOf"][0], "ConditionSet.oneOf[0]"),
        BTreeSet::from(["allOf".to_string()])
    );
    assert_eq!(
        required_set(&skip["oneOf"][1], "ConditionSet.oneOf[1]"),
        BTreeSet::from(["anyOf".to_string()])
    );

    for operator in ["allOf", "anyOf"] {
        let array = &skip["properties"][operator];
        assert_eq!(array["type"], "array");
        assert!(
            array.get("minItems").is_none(),
            "{operator} must remain lenient-admission"
        );
    }

    let atom = &skip["properties"]["allOf"]["items"];
    assert_eq!(atom["type"], "object");
    assert!(atom.get("x-kubernetes-preserve-unknown-fields").is_none());
    assert!(required_set(atom, "Condition").contains("type"));

    let clause = &skip["properties"]["anyOf"]["items"];
    assert_eq!(clause["type"], "object");
    assert!(clause.get("x-kubernetes-preserve-unknown-fields").is_none());
    assert_eq!(clause["oneOf"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        required_set(&clause["oneOf"][0], "AnyOfClause.oneOf[0]"),
        BTreeSet::from(["type".to_string()])
    );
    let forbidden_direct_fields: BTreeSet<String> = clause["oneOf"][0]["not"]["anyOf"]
        .as_array()
        .expect("AnyOfClause direct branch must structurally forbid group-only fields")
        .iter()
        .flat_map(|node| required_set(node, "AnyOfClause.oneOf[0].not.anyOf"))
        .collect();
    assert_eq!(
        forbidden_direct_fields,
        BTreeSet::from(["allOf".to_string(), "name".to_string()])
    );
    assert_eq!(
        required_set(&clause["oneOf"][1], "AnyOfClause.oneOf[1]"),
        BTreeSet::from(["allOf".to_string()])
    );
    assert_eq!(
        required_set(&clause["oneOf"][1]["not"], "AnyOfClause.oneOf[1].not"),
        BTreeSet::from(["type".to_string()])
    );
    assert!(clause.get("x-kubernetes-validations").is_none());
    assert!(
        clause["properties"].get("anyOf").is_none(),
        "AnyOfClause must not recurse"
    );
    let nested = &clause["properties"]["allOf"];
    assert_eq!(nested["type"], "array");
    assert!(
        nested.get("minItems").is_none(),
        "nested allOf must remain lenient-admission"
    );
    assert!(required_set(&nested["items"], "AllOfGroup.allOf.items").contains("type"));
    assert!(
        nested["items"]["properties"].get("allOf").is_none(),
        "nested allOf must contain atoms only"
    );
    assert!(
        nested["items"]["properties"].get("anyOf").is_none(),
        "nested allOf must contain atoms only"
    );
}

#[test]
fn direct_and_nested_atomic_condition_coverage_match() {
    let (rust_types, rust_fields) = rust_condition_sets();
    let direct = crd_any_of_clause_properties();
    let direct_types = crd_enum_values(&direct["type"]["enum"], "AnyOfClause properties.type.enum");
    let direct_fields: BTreeSet<String> = direct
        .keys()
        .filter(|key| !matches!(key.as_str(), "type" | "name" | "allOf"))
        .cloned()
        .collect();
    let direct_cov = compare_condition_coverage(&rust_types, &rust_fields, &direct_types, &direct_fields);
    assert!(direct_cov.is_clean(), "direct anyOf atom: {}", direct_cov.report());

    let nested_props = crd_conditions_node()["properties"]["skip"]["properties"]["anyOf"]["items"]["properties"]
        ["allOf"]["items"]["properties"]
        .as_object()
        .expect("nested allOf Condition properties must be an object")
        .clone();
    let nested_types = crd_enum_values(&nested_props["type"]["enum"], "nested Condition properties.type.enum");
    let nested_fields: BTreeSet<String> = nested_props.keys().filter(|key| *key != "type").cloned().collect();
    let nested_cov = compare_condition_coverage(&rust_types, &rust_fields, &nested_types, &nested_fields);
    assert!(nested_cov.is_clean(), "nested allOf atom: {}", nested_cov.report());
}

#[test]
fn conditions_schema_anchor_is_reused_at_all_five_sites() {
    // Source-text check: anchors expand on parse, so copy-paste and alias look
    // identical in the parsed document. The five `*conditionsSchema` aliases
    // are dyeRule.conditions, body.conditions, upstreamResponseFilterPlugins,
    // upstreamResponseBodyFilterPlugins, and upstreamResponsePlugins.
    let text = plugins_crd_text();
    let defs = text.matches("&conditionsSchema").count();
    let aliases = text.matches("*conditionsSchema").count();
    assert_eq!(
        defs, 1,
        "&conditionsSchema must be defined exactly once in edgion_plugins_crd.yaml, found {defs}"
    );
    assert_eq!(
        aliases, 5,
        "*conditionsSchema must be aliased at exactly five sites in edgion_plugins_crd.yaml, \
         found {aliases}"
    );

    for (anchor, expected_defs, expected_aliases) in [
        ("conditionSetSchema", 1, 1),
        ("conditionSchema", 1, 1),
        ("anyOfClauseSchema", 1, 0),
        ("allOfGroupSchema", 1, 0),
    ] {
        assert_eq!(
            text.matches(&format!("&{anchor}")).count(),
            expected_defs,
            "&{anchor} definition count drifted"
        );
        assert_eq!(
            text.matches(&format!("*{anchor}")).count(),
            expected_aliases,
            "*{anchor} alias count drifted"
        );
    }
}

#[test]
fn direction1_reports_negate_missing_from_crd() {
    let (rust_types, mut rust_fields) = rust_condition_sets();
    let (crd_types, mut crd_fields) = crd_condition_sets();
    // PROGRAM: a fixture CRD with `negate` removed. Insert on the rust side
    // (no-op once S7 lands the field) and strip it from the CRD set so the
    // assertion stays Direction-1 after both sides carry `negate`.
    rust_fields.insert("negate".to_string());
    crd_fields.remove("negate");
    let cov = compare_condition_coverage(&rust_types, &rust_fields, &crd_types, &crd_fields);
    assert!(!cov.is_clean(), "synthetic negate must produce Direction-1 drift");
    let report = cov.report();
    assert!(
        report.contains("negate"),
        "Direction-1 failure must name negate, got {report}"
    );
    assert!(
        cov.d1_fields.contains("negate"),
        "Direction-1 field set must contain negate, got {:?}",
        cov.d1_fields
    );
}

#[test]
fn direction2_reports_unbacked_crd_field() {
    let (rust_types, rust_fields) = rust_condition_sets();
    let (crd_types, mut crd_fields) = crd_condition_sets();
    crd_fields.insert("bogusField".to_string());
    let cov = compare_condition_coverage(&rust_types, &rust_fields, &crd_types, &crd_fields);
    assert!(!cov.is_clean(), "synthetic bogusField must produce Direction-2 drift");
    let report = cov.report();
    assert!(
        report.contains("bogusField"),
        "Direction-2 failure must name bogusField, got {report}"
    );
    assert!(
        cov.d2_fields.contains("bogusField"),
        "Direction-2 field set must contain bogusField, got {:?}",
        cov.d2_fields
    );
}
