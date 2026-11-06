use super::*;
use edgion_resources::output::LinkSysRef;

fn make_ref(ns: &str, n: &str) -> LinkSysRef {
    LinkSysRef {
        namespace: ns.to_string(),
        name: n.to_string(),
    }
}

#[test]
#[cfg(feature = "kafka")]
fn test_kafka_log_sender_name_returns_key() {
    let sender = KafkaLogSender::new(make_ref("default", "kafka-main"), "edgion-access".to_string()).unwrap();
    assert_eq!(sender.name(), "default/kafka-main");
}

#[test]
#[cfg(feature = "kafka")]
fn test_kafka_log_sender_topic_stored() {
    let sender = KafkaLogSender::new(make_ref("ns", "name"), "my-topic".to_string()).unwrap();
    assert_eq!(sender.topic, "my-topic");
    assert_eq!(sender.key, "ns/name");
}

#[test]
fn programmatic_output_reference_is_checked_before_sender_construction() {
    for (namespace, name) in [
        ("", "sink"),
        ("ns", ""),
        ("bad/ns", "sink"),
        ("ns", "bad/name"),
        ("NS", "sink"),
    ] {
        let reference = edgion_resources::output::LinkSysRef {
            namespace: namespace.into(),
            name: name.into(),
        };
        let error = KafkaLogSender::new(reference, "topic".to_string())
            .err()
            .expect("invalid identity must fail");
        assert_eq!(error.to_string(), "invalid LinkSys reference");
    }
    #[cfg(feature = "kafka")]
    {
        let reference = edgion_resources::output::LinkSysRef {
            namespace: "r6f-output".into(),
            name: "missing.sink".into(),
        };
        let sender = KafkaLogSender::new(reference, "topic".to_string()).unwrap();
        assert_eq!(sender.name(), "r6f-output/missing.sink");
        assert!(!sender.healthy());
    }
}

#[test]
#[cfg(not(feature = "kafka"))]
fn compiled_out_kafka_rejects_a_valid_reference() {
    let error = KafkaLogSender::new(make_ref("ns", "name"), "topic".to_string())
        .err()
        .expect("a build without the kafka feature must not construct a sender");
    assert_eq!(error.to_string(), super::super::client::KAFKA_NOT_COMPILED);
}
