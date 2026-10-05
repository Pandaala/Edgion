//! Guard the structural WAF request-body subtree against Kubernetes pruning.

use std::collections::BTreeSet;

use edgion_resources::resources::edgion_plugins::WafRequestBodyConfig;
use serde_json::Value;

fn crd() -> Value {
    serde_yaml::from_str(include_str!("../../config/crd/edgion-crd/edgion_plugins_crd.yaml")).unwrap()
}

fn normalized<'a>(mut node: &'a Value, root: &'a Value) -> &'a Value {
    loop {
        if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
            node = root.pointer(reference.strip_prefix('#').unwrap()).unwrap();
        } else if let Some(branches) = node.get("anyOf").and_then(Value::as_array) {
            node = branches.iter().find(|branch| branch["type"] != "null").unwrap();
        } else {
            return node;
        }
    }
}

fn names(node: &Value, key: &str) -> BTreeSet<String> {
    match key {
        "properties" => node[key].as_object().unwrap().keys().cloned().collect(),
        "required" => node[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|name| name.as_str().unwrap().to_string())
            .collect(),
        _ => unreachable!(),
    }
}

fn compare(expected: &Value, actual: &Value, root: &Value, path: &str) {
    let expected = normalized(expected, root);
    let expected_type = match &expected["type"] {
        Value::Array(types) => types.iter().find(|kind| **kind != "null").unwrap(),
        kind => kind,
    };
    assert_eq!(expected_type, &actual["type"], "type at {path}");
    assert_eq!(
        expected["minimum"].as_f64(),
        actual["minimum"].as_f64(),
        "minimum at {path}"
    );
    if expected_type == "object" {
        assert_eq!(
            names(expected, "properties"),
            names(actual, "properties"),
            "fields at {path}"
        );
        assert_eq!(
            names(expected, "required"),
            names(actual, "required"),
            "required at {path}"
        );
        for (name, child) in expected["properties"].as_object().unwrap() {
            compare(child, &actual["properties"][name], root, &format!("{path}.{name}"));
        }
    }
}

#[test]
fn request_body_fields_types_and_required_budgets_match_every_crd_version() {
    let root = serde_json::to_value(schemars::schema_for!(WafRequestBodyConfig)).unwrap();
    let crd = crd();
    let versions = crd["spec"]["versions"].as_array().unwrap();
    assert!(!versions.is_empty());
    for version in versions {
        let body = &version["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["waf"]["properties"]
            ["requestBody"];
        compare(&root, body, &root, "spec.waf.requestBody");
        let json = &body["properties"]["json"];
        assert!(json.get("default").is_none(), "JSON activation must stay explicit");
        for group in json["properties"].as_object().unwrap().values() {
            assert_eq!(names(group, "properties"), names(group, "required"));
            for budget in group["properties"].as_object().unwrap().values() {
                assert_eq!(budget["type"], "integer");
                assert_eq!(budget["minimum"], 0);
                assert!(budget.get("default").is_none(), "budgets must stay explicit");
            }
        }
    }
}
