use super::*;

#[test]
fn policy_has_typed_duration_and_rejects_aliases_unknowns_and_unsafe_variation() {
    let mut config: ResponseCacheConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(config.validate_and_init().unwrap(), Duration::from_secs(60));
    for json in ["{\"maxTtl\":1}", "{\"maxTtlMs\":1000}", "{\"unexpected\":true}"] {
        assert!(serde_json::from_str::<ResponseCacheConfig>(json).is_err());
    }
    for ttl in ["bad", "0s", "25h"] {
        config.max_ttl = ttl.into();
        assert!(config.validation_error().is_some());
    }
    for name in ["Authorization", "Cookie", "Connection", "traceparent", "x-trace-id"] {
        let config = ResponseCacheConfig {
            vary: vec![KeyGet::Header { name: name.into() }],
            ..Default::default()
        };
        assert!(config.validation_error().is_some());
    }
    let mut config = ResponseCacheConfig {
        vary: vec![KeyGet::Header {
            name: "Accept-Language".into(),
        }],
        ..Default::default()
    };
    config.validate_and_init().unwrap();
    assert!(matches!(&config.vary[0], KeyGet::Header { name } if name == "accept-language"));
}
