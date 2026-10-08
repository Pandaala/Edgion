use super::*;
use serde_json::json;

#[test]
fn parameters_require_exact_cluster_target_and_valid_name() {
    let mut class: GatewayClass = serde_json::from_value(json!({"metadata":{"name":"class"},"spec":{"controllerName":"example.net/controller","parametersRef":{"group":"edgion.io","kind":"EdgionGatewayConfig","name":"config"}}})).unwrap();
    let reference = GatewayClassParametersRef::from_class(&class).unwrap().unwrap();
    assert_eq!(reference.target().name(), "config");
    assert_eq!(reference.target().scope(), ResourceScope::Cluster);
    for (field, value, expected) in [
        ("group", "", ReferenceError::UnsupportedTarget),
        ("group", "core", ReferenceError::UnsupportedTarget),
        ("kind", "Secret", ReferenceError::UnsupportedTarget),
        ("kind", "", ReferenceError::UnsupportedTarget),
        ("name", "", ReferenceError::InvalidName),
        ("name", "bad/name", ReferenceError::InvalidName),
        ("namespace", "", ReferenceError::InvalidNamespace),
        ("namespace", "default", ReferenceError::InvalidNamespace),
    ] {
        let mut raw = serde_json::to_value(&class).unwrap();
        raw["spec"]["parametersRef"][field] = json!(value);
        let invalid = serde_json::from_value(raw).unwrap();
        assert_eq!(GatewayClassParametersRef::from_class(&invalid).err(), Some(expected));
    }
    class.spec.parameters_ref = None;
    class.metadata.name = None;
    class.metadata.namespace = Some("historical-fixture".into());
    assert!(
        GatewayClassParametersRef::from_class(&class).unwrap().is_none(),
        "owner metadata admission is independent; absent parameters preserve defaults"
    );
}
