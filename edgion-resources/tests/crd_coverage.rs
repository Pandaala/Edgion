//! Struct↔CRD field-coverage guard for the two fully-structural CRDs
//! (EdgionGatewayConfig, EdgionTls). See
//! docs/superpowers/specs/2026-06-15-crd-struct-coverage-guard-design.md.
//!
//! The walk is a lockstep node comparison: it descends EXPECTED (schemars) and
//! ACTUAL (CRD) together, comparing property NAME sets at each node, and reports
//! a difference at its highest absent ancestor. It is correct ONLY for resources
//! with no tagged enums / `#[serde(flatten)]` / preserve-unknown nodes — which is
//! exactly why its scope is gateway_config + tls (see the spec's Precondition).

use schemars::schema_for;
use serde_json::{Map, Value};
use std::path::PathBuf;

/// Follow a single `#/$defs/Name` `$ref` into `defs`; non-ref nodes pass through.
fn resolve<'a>(node: &'a Value, defs: &'a Map<String, Value>) -> &'a Value {
    if let Some(r) = node.get("$ref").and_then(Value::as_str) {
        if let Some(name) = r.strip_prefix("#/$defs/") {
            if let Some(target) = defs.get(name) {
                return target;
            }
        }
    }
    node
}
// NOTE: follows exactly one hop. If a $defs entry is itself a $ref (a chain
// A->B->C), the intermediate node is returned and treated as a leaf. Schemars
// 1.2.1 never emits such chains for plain structs, so this is fine for the
// current scope (gateway_config + tls); revisit if the schema generator changes.

/// `Option<T>` becomes `{"anyOf":[<T>, {"type":"null"}]}` in schemars 1.2.1.
/// Return the non-null branch.
fn unwrap_nullable(node: &Value) -> &Value {
    if let Some(arr) = node.get("anyOf").and_then(Value::as_array) {
        for m in arr {
            if m.get("type").and_then(Value::as_str) != Some("null") {
                return m;
            }
        }
    }
    node
}

/// The `properties` map of an object node, descending through array `items`
/// as a transparent hop. `None` for a leaf (scalar, enum-of-const, etc.).
fn properties<'a>(node: &'a Value, defs: &'a Map<String, Value>) -> Option<&'a Map<String, Value>> {
    let node = resolve(unwrap_nullable(node), defs);
    if let Some(items) = node.get("items") {
        return properties(items, defs);
    }
    node.get("properties").and_then(Value::as_object)
}

/// Lockstep diff. `d1` = expected-only (struct field missing from CRD);
/// `d2` = actual-only (CRD field with no struct backing). Reports at the
/// highest absent ancestor (stops recursing once a name is absent on one side).
fn diff(
    exp: &Value,
    act: &Value,
    path: &str,
    edefs: &Map<String, Value>,
    adefs: &Map<String, Value>,
    d1: &mut Vec<String>,
    d2: &mut Vec<String>,
) {
    let (ep, ap) = match (properties(exp, edefs), properties(act, adefs)) {
        (Some(ep), Some(ap)) => (ep, ap),
        _ => return, // at least one side is a leaf → stop
    };
    for (name, esub) in ep {
        match ap.get(name) {
            Some(asub) => diff(esub, asub, &format!("{path}.{name}"), edefs, adefs, d1, d2),
            None => d1.push(format!("{path}.{name}")),
        }
    }
    for name in ap.keys() {
        if !ep.contains_key(name) {
            d2.push(format!("{path}.{name}"));
        }
    }
}

/// Run the lockstep diff for one resource's `spec`, applying a Direction-1
/// allowlist of `(path)` entries that are intentionally absent from the CRD.
fn compare_spec(expected_root: &Value, crd_spec_schema: &Value, allow_d1: &[&str]) -> (Vec<String>, Vec<String>) {
    let empty = Map::new();
    let edefs = expected_root
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (mut d1, mut d2) = (Vec::new(), Vec::new());
    diff(expected_root, crd_spec_schema, "spec", &edefs, &empty, &mut d1, &mut d2);
    d1.retain(|p| !allow_d1.contains(&p.as_str()));
    d1.sort();
    d2.sort();
    (d1, d2)
}

#[test]
fn engine_detects_missing_stale_anyof_ref_and_items() {
    use serde_json::json;
    let expected = json!({
        "$defs": {
            "Inner": { "properties": { "x": { "type": "string" }, "y": { "type": "string" } } },
            "Item":  { "properties": { "n": { "type": "string" } } }
        },
        "properties": {
            "a": { "type": "string" },
            "opt": { "anyOf": [ { "$ref": "#/$defs/Inner" }, { "type": "null" } ] },
            "list": { "type": "array", "items": { "$ref": "#/$defs/Item" } },
            "missing": { "type": "string" }
        }
    });
    let actual = json!({
        "properties": {
            "a": { "type": "string" },
            "opt": { "type": "object", "properties": { "x": { "type": "string" } } },
            "list": { "type": "array", "items": { "type": "object", "properties": { "n": { "type": "string" } } } },
            "stale": { "type": "string" }
        }
    });
    let (d1, d2) = compare_spec(&expected, &actual, &[]);
    assert_eq!(d1, vec!["spec.missing".to_string(), "spec.opt.y".to_string()]);
    assert_eq!(d2, vec!["spec.stale".to_string()]);
}

#[test]
fn engine_allowlist_suppresses_named_path() {
    use serde_json::json;
    let expected = json!({ "properties": { "dep": { "type": "string" } } });
    let actual = json!({ "properties": {} });
    let (d1, _d2) = compare_spec(&expected, &actual, &["spec.dep"]);
    assert!(d1.is_empty(), "allowlisted path must be suppressed, got {d1:?}");
}

/// Load a committed CRD YAML and return its `spec.versions[0].schema.openAPIV3Schema`
/// `properties.<section>` node (`section` = "spec" or "status") as JSON.
fn crd_section(file: &str, section: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../config/crd/edgion-crd")
        .join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let doc: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let result = doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"][section].clone();
    assert!(
        result.is_object(),
        "CRD {file}: spec.versions[0].schema.openAPIV3Schema.properties.{section} \
         is not an object (got {result:?}); the CRD YAML path may have changed"
    );
    result
}

#[test]
fn gateway_config_spec_has_no_uncovered_fields() {
    let expected = serde_json::to_value(schema_for!(edgion_resources::EdgionGatewayConfigSpec)).unwrap();
    let actual = crd_section("edgion_gateway_config_crd.yaml", "spec");
    // Deprecated, intentionally absent from the CRD (back-compat detection only).
    let allow = ["spec.httpTimeout.backend.defaultMaxRetries"];
    let (d1, d2) = compare_spec(&expected, &actual, &allow);
    assert!(
        d1.is_empty() && d2.is_empty(),
        "EdgionGatewayConfig CRD drift:\n  missing from CRD (Direction-1): {d1:?}\n  stale in CRD (Direction-2): {d2:?}\n\
         Fix: add the missing field(s) to edgion_gateway_config_crd.yaml, or add an intentional omission to the allowlist with a reason."
    );
}

#[test]
fn tls_spec_has_no_uncovered_fields() {
    let expected = serde_json::to_value(schema_for!(edgion_resources::EdgionTlsSpec)).unwrap();
    let actual = crd_section("edgion_tls_crd.yaml", "spec");
    let (d1, d2) = compare_spec(&expected, &actual, &[]);
    assert!(
        d1.is_empty() && d2.is_empty(),
        "EdgionTls CRD drift:\n  missing from CRD (Direction-1): {d1:?}\n  stale in CRD (Direction-2): {d2:?}\n\
         Fix: add the missing field(s) to edgion_tls_crd.yaml, or add an intentional omission to the allowlist with a reason."
    );
}

/// Narrow regression pins for the two one-time fixes on tagged-enum CRDs that
/// the general guard intentionally does not cover (acme status, stream spec).
fn has_property(section: &Value, dotted: &str) -> bool {
    let empty = Map::new();
    let mut node = section;
    for seg in dotted.split('.') {
        match properties(node, &empty).and_then(|p| p.get(seg)) {
            Some(child) => node = child,
            None => return false,
        }
    }
    true
}

#[test]
fn acme_status_declares_active_challenges() {
    let status = crd_section("edgion_acme_crd.yaml", "status");
    assert!(
        has_property(&status, "activeChallenges.token"),
        "acme CRD status must declare activeChallenges (live HTTP-01 pruning bug)"
    );
    let spec = crd_section("edgion_acme_crd.yaml", "spec");
    assert!(
        !has_property(&spec, "activeChallenges"),
        "acme CRD spec must NOT declare activeChallenges (it is a status field)"
    );
}

#[test]
fn stream_spec_declares_tls_route_plugins() {
    let spec = crd_section("edgion_stream_plugins_crd.yaml", "spec");
    assert!(
        has_property(&spec, "tlsRoutePlugins.type"),
        "stream CRD spec must declare tlsRoutePlugins (else TLS-route IP restriction is silently pruned)"
    );
}
