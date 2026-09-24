use super::*;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
use edgion_resources::resources::link_sys::LinkSysSpec;

fn kafka_ls(brokers: Vec<&str>, linger_ms: Option<u64>, rv: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some("fp".to_string()),
            name: Some("kafka".to_string()),
            resource_version: rv.map(str::to_string),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Kafka(KafkaClientConfig {
                brokers: brokers.into_iter().map(str::to_string).collect(),
                sasl: None,
                tls: None,
                channel_size: None,
                max_topics: None,
                max_pending_records: None,
                max_pending_bytes: None,
                linger_ms,
            }),
        },
        status: None,
    }
}

#[test]
fn broker_order_does_not_change_fingerprint() {
    let left = kafka_ls(vec!["b.example:9092", "a.example:9092"], None, None);
    let right = kafka_ls(vec!["a.example:9092", "b.example:9092"], None, None);
    assert!(fingerprint_kafka(&left)
        .unwrap()
        .equal_runtime(&fingerprint_kafka(&right).unwrap()));
}

#[test]
fn defaulted_linger_matches_explicit_default() {
    let base = kafka_ls(vec!["a.example:9092"], None, None);
    let explicit = kafka_ls(vec!["a.example:9092"], Some(DEFAULT_KAFKA_LINGER_MS), None);
    assert!(fingerprint_kafka(&base)
        .unwrap()
        .equal_runtime(&fingerprint_kafka(&explicit).unwrap()));
}

#[test]
fn linger_change_rebuilds() {
    let a = kafka_ls(vec!["a.example:9092"], Some(5), None);
    let b = kafka_ls(vec!["a.example:9092"], Some(10), None);
    assert!(!fingerprint_kafka(&a)
        .unwrap()
        .equal_runtime(&fingerprint_kafka(&b).unwrap()));
}

#[test]
fn sasl_username_is_in_rebuild_hash() {
    let mut a = kafka_ls(vec!["a.example:9092"], None, None);
    let mut b = a.clone();
    if let SystemConfig::Kafka(cfg) = &mut a.spec.config {
        cfg.sasl = Some(KafkaSaslConfig {
            username: "alice".to_string(),
            password: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "creds".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        });
    }
    if let SystemConfig::Kafka(cfg) = &mut b.spec.config {
        cfg.sasl = Some(KafkaSaslConfig {
            username: "bob".to_string(),
            password: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "creds".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        });
    }
    assert!(!fingerprint_kafka(&a)
        .unwrap()
        .equal_runtime(&fingerprint_kafka(&b).unwrap()));
}
