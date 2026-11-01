//! Struct↔CRD property-presence guards for all nine custom CRDs.
//!
//! The walk is a lockstep node comparison: it descends EXPECTED (schemars) and
//! ACTUAL (CRD) together, comparing property NAME sets at each node, and reports
//! a difference at its highest absent ancestor. Fixed-field projection merges
//! schema composition branches by property name, following refs and array items.
//! Explicit variant-config boundaries remain opaque; discriminators are compared
//! by name only. Types, defaults, bounds and branch validity have separate guards.

use schemars::schema_for;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
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
// Single-hop accessor for the narrow shape/constraint assertions below. The
// field-presence comparator uses fixed_projection for chained refs and siblings.

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
    let (Some(ep), Some(ap)) = (properties(exp, edefs), properties(act, adefs)) else {
        return; // at least one side is a leaf → stop
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
    let (mut d1, d2) = compare_fixed_fields(expected_root, crd_spec_schema, "spec", &[]);
    d1.retain(|p| !allow_d1.contains(&p.as_str()));
    (d1, d2)
}

/// Merge property trees, not schema constraints. A flattened enum's named fields
/// are the union of its branches; this does not validate which branch is legal.
fn merge_projection(target: &mut Value, source: Value) {
    let target = target["properties"].as_object_mut().unwrap();
    for (name, child) in source["properties"].as_object().unwrap() {
        match target.get_mut(name) {
            Some(existing) => merge_projection(existing, child.clone()),
            None => {
                target.insert(name.clone(), child.clone());
            }
        }
    }
}

/// Project only fixed property names. Open bodies and dynamic map keys are not
/// structural fields. Do not infer a boundary from preserve-unknown: a node may
/// preserve unknown fields while still declaring fixed children that need checks.
fn fixed_projection(node: &Value, defs: &Map<String, Value>, path: &str, opaque: &[&str], depth: usize) -> Value {
    assert!(depth < 128, "recursive or excessively deep schema at {path}");
    let mut projected = serde_json::json!({"properties": {}});
    if matches_boundary(path, opaque) {
        return projected;
    }
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("unsupported schema reference {reference} at {path}"));
        let target = defs
            .get(name)
            .unwrap_or_else(|| panic!("unresolved schema reference {reference} at {path}"));
        merge_projection(&mut projected, fixed_projection(target, defs, path, opaque, depth + 1));
    }
    if let Some(items) = node.get("items") {
        merge_projection(&mut projected, fixed_projection(items, defs, path, opaque, depth + 1));
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            let child = fixed_projection(child, defs, &format!("{path}.{name}"), opaque, depth + 1);
            let mut property = serde_json::json!({"properties": {}});
            property["properties"]
                .as_object_mut()
                .unwrap()
                .insert(name.clone(), child);
            merge_projection(&mut projected, property);
        }
    }
    for keyword in ["allOf", "oneOf", "anyOf"] {
        if let Some(branches) = node.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                merge_projection(&mut projected, fixed_projection(branch, defs, path, opaque, depth + 1));
            }
        }
    }
    projected
}

/// `*` matches exactly one property segment, never an arbitrary subtree.
fn matches_boundary(path: &str, boundaries: &[&str]) -> bool {
    boundaries.iter().any(|pattern| {
        let mut pattern = pattern.split('.');
        for segment in path.split('.') {
            match pattern.next() {
                Some("*") => {}
                Some(expected) if expected == segment => {}
                _ => return false,
            }
        }
        pattern.next().is_none()
    })
}

fn compare_fixed_fields(
    expected: &Value,
    actual: &Value,
    section: &str,
    opaque: &[&str],
) -> (Vec<String>, Vec<String>) {
    let empty = Map::new();
    let defs = expected.get("$defs").and_then(Value::as_object).unwrap_or(&empty);
    let expected = fixed_projection(expected, defs, section, opaque, 0);
    let actual = fixed_projection(actual, &empty, section, opaque, 0);
    let (mut missing, mut stale) = (Vec::new(), Vec::new());
    diff(&expected, &actual, section, &empty, &empty, &mut missing, &mut stale);
    missing.sort();
    stale.sort();
    (missing, stale)
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
    let doc: Value = yaml_serde::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let result = doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"][section].clone();
    assert!(
        result.is_object(),
        "CRD {file}: spec.versions[0].schema.openAPIV3Schema.properties.{section} \
         is not an object (got {result:?}); the CRD YAML path may have changed"
    );
    result
}

struct FixedFieldScope {
    file: &'static str,
    spec: Value,
    status: Value,
    opaque: &'static [&'static str],
}

// This partially declared, preserved ParentReference still checks its fixed name.
// These optional fields survive admission without individual declarations.
const ACME_PRESERVED_REFERENCE_FIELDS: &[&str] = &[
    "spec.challenge.gatewayRef.group",
    "spec.challenge.gatewayRef.kind",
    "spec.challenge.gatewayRef.namespace",
    "spec.challenge.gatewayRef.port",
    "spec.challenge.gatewayRef.sectionName",
];

impl FixedFieldScope {
    fn compare(&self, section: &str, expected: &Value, actual: &Value, opaque: &[&str]) -> (Vec<String>, Vec<String>) {
        let (mut missing, stale) = compare_fixed_fields(expected, actual, section, opaque);
        if self.file == "edgion_acme_crd.yaml" && section == "spec" {
            assert_eq!(
                actual["properties"]["challenge"]["properties"]["gatewayRef"]["x-kubernetes-preserve-unknown-fields"],
                true,
                "ACME Gateway reference omissions require preserved unknown fields"
            );
            missing.retain(|path| !ACME_PRESERVED_REFERENCE_FIELDS.contains(&path.as_str()));
        }
        (missing, stale)
    }
}

/// Explicit config and undeclared reference bodies are excluded. Partially
/// declared preserved references still compare their fixed named children.
fn fixed_field_scopes() -> Vec<FixedFieldScope> {
    use edgion_resources::*;
    macro_rules! scope {
        ($file:literal, $spec:ty, $status:ty, $opaque:expr) => {
            FixedFieldScope {
                file: $file,
                spec: serde_json::to_value(schema_for!($spec)).unwrap(),
                status: serde_json::to_value(schema_for!($status)).unwrap(),
                opaque: $opaque,
            }
        };
    }
    vec![
        scope!(
            "edgion_gateway_config_crd.yaml",
            EdgionGatewayConfigSpec,
            EdgionGatewayConfigStatus,
            &[]
        ),
        scope!("edgion_tls_crd.yaml", EdgionTlsSpec, EdgionTlsStatus, &[]),
        scope!(
            "edgion_backend_traffic_policy_crd.yaml",
            EdgionBackendTrafficPolicySpec,
            EdgionBackendTrafficPolicyStatus,
            &[]
        ),
        scope!("edgion_backend_crd.yaml", EdgionBackendSpec, EdgionBackendStatus, &[]),
        scope!(
            "edgion_acme_crd.yaml",
            EdgionAcmeSpec,
            EdgionAcmeStatus,
            &["spec.autoEdgionTls.parentRefs"]
        ),
        scope!(
            "edgion_plugins_crd.yaml",
            EdgionPluginsSpec,
            EdgionPluginsStatus,
            &[
                "spec.requestPlugins.config",
                "spec.upstreamResponseFilterPlugins.config",
                "spec.upstreamResponseBodyFilterPlugins.config",
                "spec.upstreamResponsePlugins.config",
                // These shared references are preserved objects with no fixed
                // children declared. Their containing fields remain compared.
                "spec.*.conditions.*.*.ipRefs",
                "spec.*.conditions.*.*.secretRef",
                "spec.*.conditions.*.*.secretGroups",
                "spec.*.conditions.*.*.*.ipRefs",
                "spec.*.conditions.*.*.*.secretRef",
                "spec.*.conditions.*.*.*.secretGroups",
                "spec.*.body.conditions.*.*.ipRefs",
                "spec.*.body.conditions.*.*.secretRef",
                "spec.*.body.conditions.*.*.secretGroups",
                "spec.*.body.conditions.*.*.*.ipRefs",
                "spec.*.body.conditions.*.*.*.secretRef",
                "spec.*.body.conditions.*.*.*.secretGroups",
                "spec.*.dye.*.conditions.*.*.ipRefs",
                "spec.*.dye.*.conditions.*.*.secretRef",
                "spec.*.dye.*.conditions.*.*.secretGroups",
                "spec.*.dye.*.conditions.*.*.*.ipRefs",
                "spec.*.dye.*.conditions.*.*.*.secretRef",
                "spec.*.dye.*.conditions.*.*.*.secretGroups",
            ]
        ),
        scope!(
            "edgion_stream_plugins_crd.yaml",
            EdgionStreamPluginsSpec,
            EdgionStreamPluginsStatus,
            &["spec.plugins.config", "spec.tlsRoutePlugins.config",]
        ),
        scope!("link_sys_crd.yaml", LinkSysSpec, LinkSysStatus, &["spec.config"]),
        scope!(
            "edgion_config_data_crd.yaml",
            EdgionConfigDataSpec,
            EdgionConfigDataStatus,
            &["spec.data.config"]
        ),
    ]
}

fn served_schemas(file: &str) -> Vec<(String, Value)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../config/crd/edgion-crd")
        .join(file);
    let doc: Value = yaml_serde::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let schemas: Vec<_> = doc["spec"]["versions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|version| version["served"] == true)
        .map(|version| {
            let name = version["name"].as_str().unwrap().to_string();
            let schema = version["schema"]["openAPIV3Schema"].clone();
            assert!(schema.is_object(), "{file}/{name}: missing schema");
            (name, schema)
        })
        .collect();
    assert!(!schemas.is_empty(), "{file}: no served versions");
    schemas
}

fn property_node_mut<'a>(mut node: &'a mut Value, dotted: &str) -> &'a mut Value {
    for part in dotted.split('.') {
        if node.get("items").is_some() {
            node = node.get_mut("items").unwrap();
        }
        node = node["properties"]
            .get_mut(part)
            .unwrap_or_else(|| panic!("missing property {dotted}"));
    }
    node
}

fn item_node_mut(mut node: &mut Value) -> &mut Value {
    while node.get("items").is_some() {
        node = node.get_mut("items").unwrap();
    }
    node
}

fn boundary_paths(projection: &Value, path: &str, opaque: &[&str], paths: &mut Vec<String>) {
    if matches_boundary(path, opaque) {
        paths.push(path.to_string());
        return;
    }
    for (name, child) in projection["properties"].as_object().unwrap() {
        boundary_paths(child, &format!("{path}.{name}"), opaque, paths);
    }
}

#[test]
fn all_custom_crds_have_explicit_fixed_field_scopes() {
    let scopes = fixed_field_scopes();
    let covered: BTreeSet<_> = scopes.iter().map(|scope| scope.file.to_string()).collect();
    assert_eq!(covered.len(), scopes.len(), "duplicate CRD coverage scope");
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd");
    let committed: BTreeSet<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yaml"))
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert_eq!(
        covered, committed,
        "each custom CRD needs an explicit fixed-field scope"
    );
}

#[test]
fn all_custom_crd_fixed_spec_and_status_fields_match() {
    let mut failures = Vec::new();
    for scope in fixed_field_scopes() {
        for (version, schema) in served_schemas(scope.file) {
            for (section, expected, opaque) in [("spec", &scope.spec, scope.opaque), ("status", &scope.status, &[][..])]
            {
                let actual = &schema["properties"][section];
                assert!(
                    actual.get("properties").is_some(),
                    "{}/{version}.{section}: missing fixed object",
                    scope.file
                );
                let (missing, stale) = scope.compare(section, expected, actual, opaque);
                if !missing.is_empty() || !stale.is_empty() {
                    failures.push(format!(
                        "{}/{version}.{section}: missing={missing:?}, stale={stale:?}",
                        scope.file
                    ));
                }
            }
            let empty = Map::new();
            let mut boundaries = Vec::new();
            let defs = scope.spec.get("$defs").and_then(Value::as_object).unwrap_or(&empty);
            boundary_paths(
                &fixed_projection(&scope.spec, defs, "spec", scope.opaque, 0),
                "spec",
                scope.opaque,
                &mut boundaries,
            );
            for boundary in boundaries {
                let mut spec = schema["properties"]["spec"].clone();
                let node = property_node_mut(&mut spec, boundary.strip_prefix("spec.").unwrap());
                let node = if node.get("items").is_some() {
                    &node["items"]
                } else {
                    node
                };
                assert_eq!(
                    node["x-kubernetes-preserve-unknown-fields"], true,
                    "{}: {boundary} must stay open",
                    scope.file
                );
                if boundary.ends_with(".parentRefs")
                    || scope
                        .opaque
                        .iter()
                        .any(|pattern| pattern.contains('*') && matches_boundary(&boundary, &[*pattern]))
                {
                    assert!(
                        node.get("properties").is_none(),
                        "{}: {boundary} acquired fixed children; update its explicit coverage scope",
                        scope.file
                    );
                }
            }
        }
    }
    assert!(failures.is_empty(), "fixed CRD field drift:\n{}", failures.join("\n"));
}

#[test]
fn fixed_projection_detects_composed_fields_ref_chains_and_arrays() {
    use serde_json::json;
    let expected = json!({
        "$defs": {
            "Alias": {"$ref": "#/$defs/Item", "properties": {"sibling": {}}},
            "Item": {"properties": {"retained": {}, "missing": {}}}
        },
        "allOf": [
            {"properties": {"enable": {}}},
            {"oneOf": [
                {"properties": {"firstFixed": {}, "type": {}, "config": {"properties": {"variantA": {}}}}},
                {"properties": {"secondFixed": {}, "type": {}, "config": {"properties": {"variantB": {}}}}}
            ]}
        ],
        "properties": {"list": {"anyOf": [
            {"type": "null"}, {"items": {"$ref": "#/$defs/Alias"}}
        ]}}
    });
    let actual = json!({"properties": {
        "enable": {}, "firstFixed": {}, "secondFixed": {}, "type": {}, "config": {"properties": {"futureVariant": {}}},
        "list": {"items": {"properties": {"retained": {}, "sibling": {}, "stale": {}}}}
    }});
    let (missing, stale) = compare_fixed_fields(&expected, &actual, "spec", &["spec.config"]);
    assert_eq!(missing, ["spec.list.missing"]);
    assert_eq!(stale, ["spec.list.stale"]);

    let mut absent = actual.clone();
    absent["properties"].as_object_mut().unwrap().remove("config");
    assert_eq!(
        compare_fixed_fields(&expected, &absent, "spec", &["spec.config"]).0,
        ["spec.config", "spec.list.missing"],
        "an opaque body must still be declared"
    );
    let mut leaf = actual.clone();
    leaf["properties"]["list"] = json!({"type": "string"});
    assert_eq!(
        compare_fixed_fields(&expected, &leaf, "spec", &["spec.config"]).0,
        ["spec.list.missing", "spec.list.retained", "spec.list.sibling"],
        "a scalar must not hide missing object children"
    );
}

#[test]
fn explicit_boundary_patterns_match_only_complete_property_paths() {
    let pattern = &["spec.*.conditions.*.*.secretRef"];
    assert!(matches_boundary(
        "spec.requestPlugins.conditions.run.allOf.secretRef",
        pattern
    ));
    assert!(!matches_boundary(
        "spec.requestPlugins.conditions.run.allOf.secretRef.name",
        pattern
    ));
    assert!(!matches_boundary(
        "spec.requestPlugins.dye.request.conditions.run.allOf.secretRef",
        pattern
    ));
    assert!(!matches_boundary(
        "spec.requestPlugins.conditions.run.secretRef",
        pattern
    ));
}

#[test]
fn partially_declared_acme_reference_keeps_fixed_name_and_stale_checks() {
    let scope = fixed_field_scopes()
        .into_iter()
        .find(|scope| scope.file == "edgion_acme_crd.yaml")
        .unwrap();
    for (_, schema) in served_schemas(scope.file) {
        let mut spec = schema["properties"]["spec"].clone();
        let reference = property_node_mut(&mut spec, "challenge.gatewayRef");
        reference["properties"].as_object_mut().unwrap().remove("name").unwrap();
        reference["properties"]["staleFixedField"] = serde_json::json!({"type": "string"});
        let (missing, stale) = scope.compare("spec", &scope.spec, &spec, scope.opaque);
        assert!(missing.contains(&"spec.challenge.gatewayRef.name".to_string()));
        assert!(stale.contains(&"spec.challenge.gatewayRef.staleFixedField".to_string()));
        reference_preservation_failure(&scope, &schema["properties"]["spec"]);
    }
}

fn reference_preservation_failure(scope: &FixedFieldScope, spec: &Value) {
    let mut spec = spec.clone();
    property_node_mut(&mut spec, "challenge.gatewayRef")
        .as_object_mut()
        .unwrap()
        .remove("x-kubernetes-preserve-unknown-fields");
    assert!(
        std::panic::catch_unwind(|| scope.compare("spec", &scope.spec, &spec, scope.opaque)).is_err(),
        "optional undeclared fields must not be exempted once admission would prune them"
    );
}

#[test]
#[should_panic(expected = "unresolved schema reference")]
fn fixed_projection_rejects_unresolved_refs() {
    compare_fixed_fields(
        &serde_json::json!({"$ref": "#/$defs/Missing"}),
        &serde_json::json!({}),
        "spec",
        &[],
    );
}

#[test]
#[should_panic(expected = "recursive or excessively deep schema")]
fn fixed_projection_rejects_reference_cycles() {
    let expected = serde_json::json!({"$defs": {"Cycle": {"$ref": "#/$defs/Cycle"}}, "$ref": "#/$defs/Cycle"});
    compare_fixed_fields(&expected, &serde_json::json!({}), "spec", &[]);
}

fn first_nested_fixed_field(projection: &Value, prefix: &str, opaque: &[&str]) -> Option<String> {
    for (name, child) in projection["properties"].as_object().unwrap() {
        let path = format!("{prefix}.{name}");
        if matches_boundary(&path, opaque) || ACME_PRESERVED_REFERENCE_FIELDS.contains(&path.as_str()) {
            continue;
        }
        if let Some(nested) = first_nested_fixed_field(child, &path, opaque) {
            return Some(nested);
        }
        if path.matches('.').count() >= 2 {
            return Some(path);
        }
    }
    None
}

#[test]
fn committed_crd_mutations_detect_missing_and_stale_fixed_fields() {
    let empty = Map::new();
    for scope in fixed_field_scopes() {
        for (version, schema) in served_schemas(scope.file) {
            for (section, expected, opaque) in [("spec", &scope.spec, scope.opaque), ("status", &scope.status, &[][..])]
            {
                let actual = &schema["properties"][section];
                let defs = expected.get("$defs").and_then(Value::as_object).unwrap_or(&empty);
                let projection = fixed_projection(expected, defs, section, opaque, 0);
                let top = projection["properties"].as_object().unwrap().keys().next().unwrap();
                let mut paths = vec![format!("{section}.{top}")];
                if let Some(nested) = first_nested_fixed_field(&projection, section, opaque) {
                    paths.push(nested);
                } else {
                    // LinkSys spec is just type + an opaque config body.
                    assert!(
                        scope.file == "link_sys_crd.yaml" && section == "spec",
                        "{}/{version}.{section}: unexpectedly lost nested fixed fields",
                        scope.file
                    );
                }
                for path in paths {
                    let (parent, field) = path.rsplit_once('.').unwrap();
                    let mut mutated = actual.clone();
                    let parent = if parent == section {
                        &mut mutated
                    } else {
                        property_node_mut(&mut mutated, parent.strip_prefix(&format!("{section}.")).unwrap())
                    };
                    let parent = item_node_mut(parent);
                    parent["properties"].as_object_mut().unwrap().remove(field).unwrap();
                    parent["properties"]
                        .as_object_mut()
                        .unwrap()
                        .insert("staleFixedField".into(), serde_json::json!({"type": "string"}));
                    let (missing, stale) = scope.compare(section, expected, &mutated, opaque);
                    assert!(
                        missing.contains(&path),
                        "{}/{version}: failed to detect removed {path}: {missing:?}",
                        scope.file
                    );
                    let stale_path = format!("{}.staleFixedField", path.rsplit_once('.').unwrap().0);
                    assert!(
                        stale.contains(&stale_path),
                        "{}/{version}: failed to detect {stale_path}: {stale:?}",
                        scope.file
                    );
                }
            }
        }
    }
}

#[test]
fn committed_open_config_bodies_remain_opaque_but_required_in_the_schema() {
    for scope in fixed_field_scopes() {
        for (version, schema) in served_schemas(scope.file) {
            let actual = &schema["properties"]["spec"];
            let baseline = scope.compare("spec", &scope.spec, actual, scope.opaque);
            for boundary in scope.opaque.iter().filter(|boundary| !boundary.contains('*')) {
                let mut mutated = actual.clone();
                let path = boundary.strip_prefix("spec.").unwrap();
                *property_node_mut(&mut mutated, path) = serde_json::json!({
                    "type": "object", "x-kubernetes-preserve-unknown-fields": true,
                    "properties": {"futureVariantField": {"properties": {"nested": {"type": "string"}}}}
                });
                assert_eq!(
                    scope.compare("spec", &scope.spec, &mutated, scope.opaque),
                    baseline,
                    "{}/{version}: open-body internals must not affect fixed-field coverage: {boundary}",
                    scope.file
                );
                let (parent, field) = path.rsplit_once('.').unwrap_or(("", path));
                let parent = if parent.is_empty() {
                    &mut mutated
                } else {
                    property_node_mut(&mut mutated, parent)
                };
                let parent = item_node_mut(parent);
                parent["properties"].as_object_mut().unwrap().remove(field).unwrap();
                assert!(
                    scope
                        .compare("spec", &scope.spec, &mutated, scope.opaque)
                        .0
                        .contains(&boundary.to_string()),
                    "{}/{version}: missing open body must be detected: {boundary}",
                    scope.file
                );
            }
        }
    }
}

#[test]
fn gateway_config_spec_has_no_uncovered_fields() {
    let expected = serde_json::to_value(schema_for!(edgion_resources::EdgionGatewayConfigSpec)).unwrap();
    let actual = crd_section("edgion_gateway_config_crd.yaml", "spec");
    let (d1, d2) = compare_spec(&expected, &actual, &[]);
    assert!(
        d1.is_empty() && d2.is_empty(),
        "EdgionGatewayConfig CRD drift:\n  missing from CRD (Direction-1): {d1:?}\n  stale in CRD (Direction-2): {d2:?}\n\
         Fix: add the missing field(s) to edgion_gateway_config_crd.yaml, or add an intentional omission to the allowlist with a reason."
    );
}

#[test]
fn gateway_config_server_exposes_only_gateway_class_http_behavior() {
    let generated = serde_json::to_value(schema_for!(edgion_resources::EdgionGatewayConfigSpec)).unwrap();
    let defs = generated["$defs"].as_object().unwrap();
    let generated_server = properties(&generated["properties"]["server"], defs)
        .expect("generated EdgionGatewayConfig server schema must be an object");
    let committed = crd_section("edgion_gateway_config_crd.yaml", "spec");
    let committed_server = committed["properties"]["server"]["properties"]
        .as_object()
        .expect("committed EdgionGatewayConfig server schema must be an object");
    let expected = BTreeSet::from(["downstreamKeepaliveRequestLimit", "enableCompression"]);
    let generated_fields = generated_server.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let committed_fields = committed_server.keys().map(String::as_str).collect::<BTreeSet<_>>();

    assert_eq!(
        generated_fields, expected,
        "process configuration belongs to Gateway YAML"
    );
    assert_eq!(
        committed_fields, expected,
        "process configuration must not be exposed by the CRD"
    );
}

#[test]
fn gateway_plugin_policy_preserves_defaults_overrides_and_status_bounds() {
    use edgion_resources::resources::plugin_policy::PluginPolicyConfig;
    fn enum_values(schema: &Value) -> Value {
        if let Some(values) = schema.get("enum") {
            return values.clone();
        }
        Value::Array(
            schema["oneOf"]
                .as_array()
                .expect("enum must declare enum values or oneOf constants")
                .iter()
                .map(|variant| variant["const"].clone())
                .collect(),
        )
    }
    let generated = serde_json::to_value(schema_for!(PluginPolicyConfig)).unwrap();
    let defs = generated["$defs"].as_object().unwrap();
    let crd = crd_section("edgion_gateway_config_crd.yaml", "spec");
    let actual = &crd["properties"]["pluginPolicy"]["properties"];
    let expected = &generated["properties"];
    for field in ["defaultAction", "deniedAction"] {
        assert_eq!(
            actual[field]["enum"],
            enum_values(resolve(&expected[field], defs)),
            "{field}"
        );
        assert_eq!(actual[field]["default"], expected[field]["default"], "{field}");
    }
    assert_eq!(actual["deny"]["default"], serde_json::json!([]));
    assert!(
        actual["allow"].get("default").is_none(),
        "default [] would enable deny-all"
    );
    assert!(expected["allow"].get("default").is_none());
    assert_eq!(actual["blockStatus"]["default"], serde_json::json!(503));
    assert_eq!(actual["blockStatus"]["default"], expected["blockStatus"]["default"]);

    let expected_entry = resolve(&expected["deny"]["items"], defs);
    let actual_entry = &actual["deny"]["items"];
    assert_eq!(actual_entry["required"], serde_json::json!(["name"]));
    let expected_entry = &expected_entry["properties"];
    let actual_entry = &actual_entry["properties"];
    for field in ["action", "blockStatus"] {
        assert!(actual_entry[field].get("default").is_none(), "{field} must inherit");
        assert!(expected_entry[field].get("default").is_none(), "{field} must inherit");
    }
    assert_eq!(
        actual_entry["action"]["enum"],
        enum_values(resolve(unwrap_nullable(&expected_entry["action"]), defs))
    );
    for (actual_status, expected_status) in [
        (&actual["blockStatus"], &expected["blockStatus"]),
        (&actual_entry["blockStatus"], &expected_entry["blockStatus"]),
    ] {
        for (bound, value) in [("minimum", 400.0), ("maximum", 599.0)] {
            assert_eq!(actual_status[bound].as_f64(), Some(value));
            assert_eq!(expected_status[bound].as_f64(), Some(value));
        }
    }
}

#[test]
fn plugin_key_get_enum_uses_remote_ip_names() {
    let spec = crd_section("edgion_plugins_crd.yaml", "spec");
    let values = spec["properties"]["requestPlugins"]["items"]["properties"]["conditions"]["properties"]["skip"]
        ["properties"]["allOf"]["items"]["properties"]["key"]["properties"]["type"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("EdgionPlugins KeyGet enum is missing from the committed CRD"));
    let values: Vec<&str> = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("KeyGet enum value is not a string: {value:?}"))
        })
        .collect();
    assert_eq!(
        values,
        vec![
            "remoteIp",
            "directPeerIp",
            "traceId",
            "host",
            "header",
            "cookie",
            "query",
            "path",
            "method",
            "ctx",
            "remoteIpAndPath",
            "requestPath",
        ]
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

#[test]
fn tls_crd_is_recursively_structural() {
    let actual = crd_section("edgion_tls_crd.yaml", "spec");
    assert_declared_objects_are_structural(&actual, "openAPIV3Schema.properties.spec");
}

#[test]
fn tls_attachment_dimension_limits_match_runtime_and_crd() {
    let generated = serde_json::to_value(schema_for!(edgion_resources::EdgionTlsSpec)).unwrap();
    let actual = crd_section("edgion_tls_crd.yaml", "spec");
    for (field, expected) in [
        (
            "parentRefs",
            edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_PARENT_REFS,
        ),
        ("hosts", edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_HOSTS),
    ] {
        assert_eq!(generated["properties"][field]["maxItems"], expected);
        assert_eq!(actual["properties"][field]["maxItems"], expected);
    }
}

#[test]
fn backend_traffic_policy_spec_has_no_uncovered_fields() {
    let expected = serde_json::to_value(schema_for!(edgion_resources::EdgionBackendTrafficPolicySpec)).unwrap();
    let actual = crd_section("edgion_backend_traffic_policy_crd.yaml", "spec");
    let (d1, d2) = compare_spec(&expected, &actual, &[]);
    assert!(
        d1.is_empty() && d2.is_empty(),
        "EdgionBackendTrafficPolicy CRD drift:\n  missing from CRD (Direction-1): {d1:?}\n  stale in CRD (Direction-2): {d2:?}\n\
         Fix: add the missing field(s) to edgion_backend_traffic_policy_crd.yaml, or add an intentional omission to the allowlist with a reason."
    );
}

#[test]
fn backend_traffic_policy_target_ref_contract_matches_crd() {
    let generated = serde_json::to_value(schema_for!(edgion_resources::EdgionBackendTrafficPolicySpec)).unwrap();
    let generated_target_refs = &generated["properties"]["targetRefs"];
    assert_eq!(generated_target_refs["minItems"], 1);
    assert!(
        generated_target_refs["maxItems"].is_null(),
        "the generated schema must not cap Service target fan-out"
    );

    let actual = crd_section("edgion_backend_traffic_policy_crd.yaml", "spec");
    let target_refs = &actual["properties"]["targetRefs"];
    assert_eq!(target_refs["minItems"], 1);
    assert!(
        target_refs["maxItems"].is_null(),
        "the shared CRD array must not cap Service target fan-out"
    );

    let groups = string_enum_values(&target_refs["items"]["properties"]["group"]);
    assert_eq!(
        groups,
        BTreeSet::from(["".to_string(), "core".to_string(), "edgion.io".to_string()])
    );

    let kinds = string_enum_values(&target_refs["items"]["properties"]["kind"]);
    assert_eq!(
        kinds,
        BTreeSet::from(["EdgionBackend".to_string(), "Service".to_string()])
    );
}

#[test]
fn edgion_backend_spec_has_no_uncovered_fields() {
    let expected = serde_json::to_value(schema_for!(edgion_resources::EdgionBackendSpec)).unwrap();
    let actual = crd_section("edgion_backend_crd.yaml", "spec");
    let (d1, d2) = compare_spec(&expected, &actual, &[]);
    assert!(
        d1.is_empty() && d2.is_empty(),
        "EdgionBackend CRD drift:\n  missing from CRD (Direction-1): {d1:?}\n  stale in CRD (Direction-2): {d2:?}\n\
         Fix: add the missing field(s) to edgion_backend_crd.yaml, or add an intentional omission to the allowlist with a reason."
    );
    assert!(
        !contains_property_named(&expected, "secret"),
        "generated EdgionBackend schema must not expose any controller-resolved Secret"
    );
    assert!(
        !contains_property_named(&actual, "secret"),
        "EdgionBackend CRD must not expose any controller-resolved Secret"
    );
    assert!(
        actual["properties"]["ai"]["properties"]["defaults"]["properties"]
            .get("timeout")
            .is_none(),
        "EdgionBackend CRD must not retain the removed defaults.timeout field"
    );
}

#[test]
fn edgion_backend_provider_enum_matches_crd_bidirectionally() {
    let expected_schema =
        serde_json::to_value(schema_for!(edgion_resources::resources::edgion_backend::AiProvider)).unwrap();
    let expected = string_enum_values(&expected_schema);

    let actual_schema = crd_section("edgion_backend_crd.yaml", "spec");
    let actual_provider = actual_schema["properties"]["ai"]["properties"]["provider"].clone();
    let actual = string_enum_values(&actual_provider);

    let missing_from_crd: Vec<_> = expected.difference(&actual).cloned().collect();
    let stale_in_crd: Vec<_> = actual.difference(&expected).cloned().collect();
    assert!(
        missing_from_crd.is_empty() && stale_in_crd.is_empty(),
        "EdgionBackend provider enum drift:\n  missing from CRD (Direction-1): {missing_from_crd:?}\n  stale in CRD (Direction-2): {stale_in_crd:?}"
    );
}

fn contains_property_named(node: &Value, needle: &str) -> bool {
    match node {
        Value::Object(map) => {
            map.get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key(needle))
                || map.values().any(|child| contains_property_named(child, needle))
        }
        Value::Array(items) => items.iter().any(|child| contains_property_named(child, needle)),
        _ => false,
    }
}

fn assert_declared_objects_are_structural(node: &Value, path: &str) {
    match node {
        Value::Object(map) => {
            if map.get("properties").is_some_and(Value::is_object) {
                assert_ne!(
                    map.get("x-kubernetes-preserve-unknown-fields"),
                    Some(&Value::Bool(true)),
                    "declared object must not preserve unknown fields at {path}"
                );
                assert!(
                    map.get("additionalProperties").is_none(),
                    "structural schemas must not combine additionalProperties with declared properties at {path}"
                );
            }
            for (name, child) in map {
                assert_declared_objects_are_structural(child, &format!("{path}.{name}"));
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                assert_declared_objects_are_structural(child, &format!("{path}[{index}]"));
            }
        }
        _ => {}
    }
}

#[test]
fn edgion_backend_crd_is_recursively_structural_and_secret_free() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_backend_crd.yaml");
    let text = std::fs::read_to_string(&path).unwrap();
    let crd: Value = yaml_serde::from_str(&text).unwrap();
    let schema = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"];
    assert_declared_objects_are_structural(schema, "openAPIV3Schema");
    assert!(!contains_property_named(schema, "secret"));
    assert_eq!(schema["required"], serde_json::json!(["spec"]));
    assert_eq!(schema["properties"]["spec"]["required"], serde_json::json!(["ai"]));
}

fn string_enum_values(schema: &Value) -> BTreeSet<String> {
    schema["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a string enum schema, got {schema:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("expected a string enum value, got {value:?}"))
                .to_string()
        })
        .collect()
}

/// Narrow historical regression pins, retained alongside complete fixed-field
/// coverage so their original trigger and diagnostic remain independently visible.
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
fn acme_status_declares_runtime_owned_fields() {
    let status = crd_section("edgion_acme_crd.yaml", "status");
    assert!(
        has_property(&status, "activeChallenges.token"),
        "acme CRD status must declare activeChallenges (live HTTP-01 pruning bug)"
    );
    for field in [
        "scheduler.observedGeneration",
        "scheduler.attemptId",
        "scheduler.attemptsUsed",
        "scheduler.attemptStartedAt",
        "scheduler.nextRetryAt",
    ] {
        assert!(has_property(&status, field), "acme CRD status must declare {field}");
    }
    let spec = crd_section("edgion_acme_crd.yaml", "spec");
    assert!(
        !has_property(&spec, "activeChallenges"),
        "acme CRD spec must NOT declare activeChallenges (it is a status field)"
    );
}

#[test]
fn acme_challenge_crd_matches_http01_only_runtime() {
    use edgion_resources::resources::edgion_acme::AcmeChallenge;

    let spec = crd_section("edgion_acme_crd.yaml", "spec");
    let challenge = &spec["properties"]["challenge"];
    let types = challenge["properties"]["type"]["enum"].as_array().unwrap();
    assert_eq!(types, &[serde_json::json!("http-01")]);
    assert!(has_property(&spec, "challenge.gatewayRef.name"));
    assert!(
        !has_property(&spec, "resolvedListenerAttachments"),
        "controller-resolved ACME listener attachments must not be operator-visible"
    );
    assert!(!has_property(&spec, "dnsCredentialSecret"));
    assert!(!has_property(&spec, "accountSecret"));
    assert!(has_property(&spec, "privateKeySecretRef.name"));
    for kind in ["http-01", "dns-01", "tls-alpn-01"] {
        let value = serde_json::json!({"type": kind, "gatewayRef": {"name": "gateway"}});
        let runtime_accepts = serde_json::from_value::<AcmeChallenge>(value).is_ok();
        assert_eq!(runtime_accepts, types.contains(&serde_json::json!(kind)), "{kind}");
    }
}

#[test]
fn stream_spec_declares_tls_route_plugins() {
    let spec = crd_section("edgion_stream_plugins_crd.yaml", "spec");
    assert!(
        has_property(&spec, "tlsRoutePlugins.type"),
        "stream CRD spec must declare tlsRoutePlugins (else TLS-route IP restriction is silently pruned)"
    );
}

#[test]
fn linksys_auth_reference_fields_survive_kubernetes_pruning() {
    use edgion_resources::resources::common::SecretObjectReference;
    use edgion_resources::resources::link_sys::otlp::OtlpClientConfig;

    let crd = crd_section("link_sys_crd.yaml", "spec");
    let auth = &crd["properties"]["config"]["properties"]["auth"];
    let actual = auth["properties"]["secretRef"]["properties"].as_object().unwrap();
    let expected = serde_json::to_value(schema_for!(SecretObjectReference)).unwrap();
    let expected = expected["properties"].as_object().unwrap();
    assert_eq!(
        actual.keys().collect::<BTreeSet<_>>(),
        expected.keys().collect::<BTreeSet<_>>(),
        "every operator reference field must survive Kubernetes structural pruning"
    );
    assert!(
        auth["properties"].get("secret").is_none(),
        "resolved credentials must stay schema-excluded"
    );
    assert!(crd["properties"]["type"].get("enum").is_none());

    // Unsupported fields must reach the same typed validation in Kubernetes,
    // FS and etcd; pruning them would silently select a core Secret instead.
    for reference in [
        serde_json::json!({"name": "auth", "kind": "ConfigMap"}),
        serde_json::json!({"name": "auth", "group": "external.example"}),
    ] {
        for field in reference.as_object().unwrap().keys() {
            assert!(actual.contains_key(field));
        }
        let config: OtlpClientConfig = serde_json::from_value(serde_json::json!({
            "endpoint": "https://collector.example", "auth": {"secretRef": reference}
        }))
        .unwrap();
        assert!(config.validate().is_err());
    }
}

#[test]
fn linksys_removed_redis_fields_survive_kubernetes_pruning_for_advisory_reporting() {
    let crd = crd_section("link_sys_crd.yaml", "spec");
    let config = &crd["properties"]["config"];

    for path in [
        &["properties", "pool"][..],
        &["properties", "topology", "properties", "cluster"][..],
    ] {
        let mut node = config;
        for segment in path {
            node = &node[*segment];
        }
        assert_eq!(
            node["x-kubernetes-preserve-unknown-fields"],
            serde_json::json!(true),
            "removed Redis fields must reach strict ingest and become advisory unknown_fields"
        );
    }

    for (field, node) in [
        ("db", &config["properties"]["db"]),
        ("pool.size", &config["properties"]["pool"]["properties"]["size"]),
    ] {
        assert!(
            node.get("minimum").is_none() && node.get("maximum").is_none(),
            "Redis value bounds belong to lenient Controller validation: {field}"
        );
    }
    let redirects = &config["properties"]["topology"]["properties"]["cluster"]["properties"]["maxRedirects"];
    assert!(
        redirects.get("minimum").is_none() && redirects.get("maximum").is_none(),
        "Redis redirect bounds belong to lenient Controller validation"
    );
}

#[test]
fn global_real_ip_crd_preserves_only_policy_interior_and_declares_reference_shape() {
    for (version, schema) in served_schemas("edgion_gateway_config_crd.yaml") {
        let spec = &schema["properties"]["spec"];
        let policy = &spec["properties"]["realIp"];
        assert_eq!(
            policy["x-kubernetes-preserve-unknown-fields"], true,
            "{version}: removed fields must reach typed invalid capture"
        );
        assert!(
            spec.get("x-kubernetes-preserve-unknown-fields").is_none(),
            "{version}: preserve must not widen to spec"
        );
        assert!(policy["required"]
            .as_array()
            .unwrap()
            .contains(&Value::String("trustedIpRefs".into())));
        let refs = &policy["properties"]["trustedIpRefs"];
        assert_eq!(refs["type"], "array");
        assert_eq!(refs["minItems"], 1);
        let item = &refs["items"];
        assert_eq!(item["type"], "object");
        for field in ["name", "namespace"] {
            assert_eq!(item["properties"][field]["type"], "string");
            assert!(item["required"]
                .as_array()
                .unwrap()
                .contains(&Value::String(field.into())));
        }
        for forbidden in ["trustedIps", "cidrs", "invalidInput", "resolvedRefs"] {
            assert!(
                policy["properties"].get(forbidden).is_none(),
                "{version}: legacy payload and resolver metadata are not operator fields"
            );
        }
        assert!(item["properties"].get("refDenied").is_none());
        assert!(item.get("x-kubernetes-preserve-unknown-fields").is_none());
    }
}

#[test]
fn geo_reference_wire_uses_existing_opaque_plugin_boundary_and_hides_proofs() {
    for (file, stages) in [
        (
            "edgion_plugins_crd.yaml",
            vec![
                "requestPlugins",
                "upstreamResponseFilterPlugins",
                "upstreamResponseBodyFilterPlugins",
                "upstreamResponsePlugins",
            ],
        ),
        ("edgion_stream_plugins_crd.yaml", vec!["plugins"]),
    ] {
        for (version, schema) in served_schemas(file) {
            for stage in &stages {
                let config = &schema["properties"]["spec"]["properties"][*stage]["items"]["properties"]["config"];
                assert_eq!(config["type"], "object", "{file}/{version}/{stage}");
                assert_eq!(
                    config["x-kubernetes-preserve-unknown-fields"], true,
                    "removed Geo fields must reach typed-invalid ingest"
                );
            }
        }
    }
    let generated = serde_json::to_value(schema_for!(
        edgion_resources::resources::edgion_plugins::plugin_configs::GeoRule
    ))
    .unwrap();
    let fields = generated["properties"].as_object().unwrap();
    for field in ["source", "key", "values", "ipRefs"] {
        assert!(fields.contains_key(field), "{field}");
    }
    for field in ["invalidInput", "resolvedRefs", "cidrMatcher", "geoipRefs"] {
        assert!(!fields.contains_key(field), "{field}");
    }
    assert_eq!(fields["ipRefs"]["type"], "array");
    assert!(generated["$defs"]["EdgionConfigDataRef"]["properties"]
        .get("refDenied")
        .is_none());
}
