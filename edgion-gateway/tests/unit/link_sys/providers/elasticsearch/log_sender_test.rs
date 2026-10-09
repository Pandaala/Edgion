use super::*;
use edgion_resources::output::LinkSysRef;

#[test]
fn test_es_log_sender_name_returns_key() {
    let r = LinkSysRef {
        namespace: "default".to_string(),
        name: "es-main".to_string(),
    };
    let sender = EsLogSender::new(r).unwrap();
    assert_eq!(sender.name(), "default/es-main");
}

#[tokio::test]
async fn test_es_log_sender_healthy_false_when_not_registered() {
    let r = LinkSysRef {
        namespace: "default".to_string(),
        name: "nonexistent".to_string(),
    };
    let sender = EsLogSender::new(r).unwrap();
    assert!(!sender.healthy());
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
        let error = EsLogSender::new(reference).err().expect("invalid identity must fail");
        assert_eq!(error.to_string(), "invalid LinkSys reference");
    }
    let reference = edgion_resources::output::LinkSysRef {
        namespace: "r6f-output".into(),
        name: "missing.sink".into(),
    };
    let sender = EsLogSender::new(reference).unwrap();
    assert_eq!(sender.name(), "r6f-output/missing.sink");
    assert!(!sender.healthy());
}
