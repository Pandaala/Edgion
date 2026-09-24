use super::*;

#[test]
fn visibility_defaults_to_namespace_when_absent() {
    // a minimal spec JSON without `visibility`
    let spec: EdgionConfigDataSpec = serde_json::from_str(r#"{"data":{"type":"Misc","config":{}}}"#).unwrap();
    assert_eq!(spec.visibility, ConfigDataVisibility::Namespace);
}

#[test]
fn visibility_cluster_round_trips_pascal_case() {
    let spec: EdgionConfigDataSpec =
        serde_json::from_str(r#"{"visibility":"Cluster","data":{"type":"Misc","config":{}}}"#).unwrap();
    assert_eq!(spec.visibility, ConfigDataVisibility::Cluster);
    let json = serde_json::to_string(&spec).unwrap();
    assert!(json.contains(r#""visibility":"Cluster""#), "got: {json}");
}
