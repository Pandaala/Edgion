#[test]
fn hostname_validation_and_attachment_transport() {
    use super::*;
    let spec = |hosts: serde_json::Value| {
        serde_json::from_value::<TLSRouteSpec>(serde_json::json!({
            "hostnames": hosts,
            "resolvedListeners": [{"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                "gatewayName": "gateway", "listenerName": "tls", "hostnames": ["a.example.com"]}]
        }))
        .unwrap()
    };
    for hosts in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!(["*"]),
        serde_json::json!(["127.0.0.1"]),
        serde_json::json!(["A.example.com"]),
    ] {
        assert!(spec(hosts).validate_hostnames().is_err());
    }
    let value = spec(serde_json::json!(["*.example.com", "a.example.com"]));
    assert!(value.validate_hostnames().is_ok());
    let serialized = serde_json::to_value(&value).unwrap();
    assert_eq!(serialized["resolvedListeners"][0]["hostnames"][0], "a.example.com");
    assert!(serialized.get("effectiveHostnames").is_none());
    let restored: TLSRouteSpec = serde_json::from_value(serialized).unwrap();
    assert_eq!(restored.resolved_listeners, value.resolved_listeners);
    let schema = serde_json::to_value(schemars::schema_for!(TLSRouteSpec)).unwrap();
    assert!(schema["properties"].get("resolvedListeners").is_none());
    assert!(spec(serde_json::json!(vec!["a.example.com"; 1024]))
        .validate_hostnames()
        .is_ok());
    assert!(spec(serde_json::json!(vec!["a.example.com"; 1025]))
        .validate_hostnames()
        .is_err());
}
