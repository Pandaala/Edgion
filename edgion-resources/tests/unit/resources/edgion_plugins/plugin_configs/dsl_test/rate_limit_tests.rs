use super::*;

fn config_value() -> serde_json::Value {
    serde_json::json!({
        "name": "limited", "source": "redis.rate_limit(\"cache\", \"requests\", \"key\", 1)",
        "linkSysRefs": [{"name": "cache", "namespace": "default"}],
        "rateLimitPolicies": [{"name": "requests", "link": "cache", "namespace": "api/requests",
            "rate": 100, "interval": "1s", "algorithm": "FixedWindow"}]
    })
}

#[test]
fn redis_rate_limit_local_config_roundtrip_and_boundaries() {
    let config: DslConfig = serde_json::from_value(config_value()).unwrap();
    assert_eq!(config.rate_limit_policies[0].timeout_ms, 500);
    let script = config.compile_and_validate().unwrap();
    let mut transported: DslConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
    transported.source = None;
    transported.bytecode = Some(script.serialize_base64().unwrap());
    assert!(transported.compile_and_validate().is_ok());
    for (field, value) in [
        ("rate", serde_json::json!(0)),
        ("rate", serde_json::json!(1_000_001)),
        ("interval", serde_json::json!("0s")),
        ("interval", serde_json::json!("2147483648ms")),
        ("timeoutMs", serde_json::json!(0)),
        ("timeoutMs", serde_json::json!(30001)),
        ("namespace", serde_json::json!("")),
        ("namespace", serde_json::json!("{hash-tag}")),
        ("link", serde_json::json!("missing")),
        ("name", serde_json::json!("")),
    ] {
        let mut input = config_value();
        input["rateLimitPolicies"][0][field] = value;
        let invalid: DslConfig = serde_json::from_value(input).unwrap();
        assert!(invalid.compile_and_validate().is_err(), "accepted invalid {field}");
    }
    let mut duplicate = config.clone();
    duplicate
        .rate_limit_policies
        .push(config.rate_limit_policies[0].clone());
    assert!(duplicate.validate().unwrap_err().contains("duplicate"));
    let mut unknown_field = config_value();
    unknown_field["rateLimitPolicies"][0]["timoutMs"] = serde_json::json!(5);
    assert!(serde_json::from_value::<DslConfig>(unknown_field).is_err());
}
