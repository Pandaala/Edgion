use super::*;

#[test]
fn test_kafka_client_config_brokers_only_serde() {
    let config = KafkaClientConfig {
        brokers: vec!["broker1:9092".to_string(), "broker2:9092".to_string()],
        sasl: None,
        tls: None,
        channel_size: None,
        max_topics: None,
        max_pending_records: None,
        max_pending_bytes: None,
        linger_ms: None,
    };
    let json = serde_json::to_string(&config).unwrap();
    // Optional fields are skipped
    assert!(json.contains("broker1:9092"));
    assert!(!json.contains("sasl"));
    assert!(!json.contains("tls"));
}
