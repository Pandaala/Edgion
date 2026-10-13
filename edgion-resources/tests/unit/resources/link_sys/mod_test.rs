use super::*;

#[test]
fn elasticsearch_date_pattern_errors_flow_through_shared_validation() {
    for pattern in ["%", "%#z"] {
        let spec: LinkSysSpec = serde_json::from_value(serde_json::json!({
            "type": "elasticsearch", "config": {"endpoints": [], "index": {"datePattern": pattern}}
        }))
        .unwrap();
        let errors = make_link_sys("es", spec).validate_config();
        assert_eq!(
            errors,
            vec![
                "Elasticsearch configuration has no endpoints",
                "Elasticsearch index.datePattern must be a strftime pattern that supports UTC formatting",
            ]
        );
    }
    let spec: LinkSysSpec = serde_json::from_value(serde_json::json!({
        "type": "elasticsearch", "config": {"endpoints": ["http://es.example:9200"], "index": {"datePattern": "%%"}}
    }))
    .unwrap();
    assert!(make_link_sys("es", spec).validate_config().is_empty());
}

fn make_link_sys(name: &str, spec: LinkSysSpec) -> LinkSys {
    LinkSys {
        metadata: kube::api::ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

#[test]
fn http_dns_variant_roundtrips_and_validates() {
    let yaml = r#"
type: httpdns
config:
  urlTemplate: "https://203.0.113.10/d?host={domain}"
  response:
    kind: json
    ipPath: ips
  fallback:
    type: system
"#;
    let spec: LinkSysSpec = serde_yaml::from_str(yaml).expect("parse httpdns LinkSys");
    assert_eq!(spec.config.system_type(), SystemType::HttpDns);
    assert!(spec.config.as_http_dns().is_some());

    let ls = make_link_sys("h", spec);
    assert!(
        ls.validate_config().is_empty(),
        "valid httpdns must pass validate_config"
    );
}

#[test]
fn http_dns_missing_url_template_is_invalid() {
    let yaml = "type: httpdns\nconfig:\n  fallback:\n    type: none\n";
    let spec: LinkSysSpec = serde_yaml::from_str(yaml).unwrap();
    let ls = make_link_sys("h", spec);
    assert!(!ls.validate_config().is_empty());
}

#[test]
fn kafka_zero_queue_limits_are_invalid() {
    let yaml = r#"
type: kafka
config:
  brokers: ["kafka:9092"]
  channelSize: 0
  maxTopics: 0
  maxPendingRecords: 0
  maxPendingBytes: 0
"#;
    let spec: LinkSysSpec = serde_yaml::from_str(yaml).expect("parse Kafka LinkSys");
    let errors = make_link_sys("k", spec).validate_config();

    assert_eq!(errors.len(), 4);
    assert!(errors.iter().any(|error| error.contains("channelSize")));
    assert!(errors.iter().any(|error| error.contains("maxTopics")));
    assert!(errors.iter().any(|error| error.contains("maxPendingRecords")));
    assert!(errors.iter().any(|error| error.contains("maxPendingBytes")));
}

#[test]
fn redis_contract_errors_flow_through_link_sys_validation() {
    let yaml = r#"
type: redis
config:
  endpoints: ["redis://redis.example.com:6379"]
  db: 256
  pool:
    size: 0
  timeout:
    connect: 0s
    command: 31s
  topology:
    mode: cluster
    cluster:
      maxRedirects: 65
"#;
    let spec: LinkSysSpec = serde_yaml::from_str(yaml).expect("parse Redis LinkSys");
    let errors = make_link_sys("r", spec).validate_config().join("; ");

    assert!(errors.contains("db must be between 0 and 255"));
    assert!(errors.contains("pool.size must be between 1 and 64"));
    assert!(errors.contains("timeout.connect must be greater than zero"));
    assert!(errors.contains("timeout.command must not exceed 30000ms"));
    assert!(errors.contains("maxRedirects must not exceed 64"));
}

#[test]
fn kafka_linger_admission_covers_produce_deadline_boundaries() {
    for (linger, valid) in [
        (None, true),
        (Some(0), true),
        (Some(29999), true),
        (Some(30000), false),
        (Some(u64::MAX), false),
    ] {
        let spec = serde_json::from_value(serde_json::json!({
            "type": "kafka", "config": {"brokers": ["broker.example:9092"], "lingerMs": linger}
        }))
        .unwrap();
        let errors = make_link_sys("kafka", spec).validate_config();
        let expected = if valid {
            vec![]
        } else {
            vec!["Kafka lingerMs must be shorter than the 30 second produce timeout"]
        };
        assert_eq!(errors, expected, "linger {linger:?}");
    }
}

#[test]
fn kafka_validation_preserves_order_with_all_scalar_errors() {
    let spec = serde_json::from_value(serde_json::json!({
        "type": "kafka", "config": {"brokers": [], "channelSize": 0, "maxTopics": 0,
            "maxPendingRecords": 0, "maxPendingBytes": 0, "lingerMs": 30000}
    }))
    .unwrap();
    assert_eq!(
        make_link_sys("kafka", spec).validate_config(),
        vec![
            "Kafka configuration has no brokers",
            "Kafka channelSize must be greater than zero",
            "Kafka maxTopics must be greater than zero",
            "Kafka maxPendingRecords must be greater than zero",
            "Kafka maxPendingBytes must be greater than zero",
            "Kafka lingerMs must be shorter than the 30 second produce timeout",
        ]
    );
}
