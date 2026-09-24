use super::*;
use edgion_resources::output::LinkSysRef;

fn make_ref(ns: &str, n: &str) -> LinkSysRef {
    LinkSysRef {
        namespace: ns.to_string(),
        name: n.to_string(),
    }
}

#[test]
fn test_kafka_log_sender_name_returns_key() {
    let sender = KafkaLogSender::new(make_ref("default", "kafka-main"), "edgion-access".to_string());
    assert_eq!(sender.name(), "default/kafka-main");
}

#[test]
fn test_kafka_log_sender_topic_stored() {
    let sender = KafkaLogSender::new(make_ref("ns", "name"), "my-topic".to_string());
    assert_eq!(sender.topic, "my-topic");
    assert_eq!(sender.key, "ns/name");
}
