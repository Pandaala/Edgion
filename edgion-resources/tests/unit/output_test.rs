use super::*;

#[test]
fn test_link_sys_ref_key() {
    let r = LinkSysRef {
        namespace: "default".to_string(),
        name: "es-main".to_string(),
    };
    assert_eq!(r.key(), "default/es-main");
    assert_eq!(format!("{}", r), "default/es-main");
}

#[test]
fn test_string_output_es_serde_roundtrip_struct() {
    let original = StringOutput::Es {
        link_sys_ref: LinkSysRef {
            namespace: "default".to_string(),
            name: "es-main".to_string(),
        },
    };
    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(
        json,
        r#"{"es":{"linkSysRef":{"namespace":"default","name":"es-main"}}}"#
    );
    let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
    match roundtrip {
        StringOutput::Es { link_sys_ref } => {
            assert_eq!(link_sys_ref.namespace, "default");
            assert_eq!(link_sys_ref.name, "es-main");
        }
        other => panic!("unexpected variant: {:?}", other),
    }
}

#[test]
fn test_string_output_kafka_serde_roundtrip() {
    let original = StringOutput::Kafka {
        link_sys_ref: LinkSysRef {
            namespace: "default".to_string(),
            name: "kafka-main".to_string(),
        },
        topic: "edgion-access".to_string(),
    };
    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(
        json,
        r#"{"kafka":{"linkSysRef":{"namespace":"default","name":"kafka-main"},"topic":"edgion-access"}}"#
    );
    let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
    match roundtrip {
        StringOutput::Kafka { link_sys_ref, topic } => {
            assert_eq!(link_sys_ref.key(), "default/kafka-main");
            assert_eq!(topic, "edgion-access");
        }
        other => panic!("unexpected variant: {:?}", other),
    }
}

#[test]
fn test_string_output_local_file_default_unchanged() {
    let s = StringOutput::default();
    assert!(matches!(s, StringOutput::LocalFile(_)));
}

#[test]
fn test_local_file_cfg_disk_threshold_default_and_roundtrip() {
    // Default is enabled at 90%.
    let def = LocalFileWriterCfg::default();
    assert_eq!(def.disk_usage_threshold_percent, Some(90));

    // Explicit 0 (disabled) survives a round-trip.
    let cfg = LocalFileWriterCfg {
        disk_usage_threshold_percent: Some(0),
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: LocalFileWriterCfg = serde_json::from_str(&json).unwrap();
    assert_eq!(back.disk_usage_threshold_percent, Some(0));
}

#[test]
fn otlp_reference_closed_shape_and_safe_errors() {
    let value = serde_json::json!({"otlp":{"linkSysRef":{"namespace":"default","name":"collector"}}});
    let output: StringOutput = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(output).unwrap(), value);
    for namespace in ["", "Private", "private/secret", "name.space", "bad-", "-bad"] {
        let value = serde_json::json!({"namespace":namespace,"name":"collector"});
        let error = serde_json::from_value::<LinkSysRef>(value).unwrap_err();
        assert_eq!(error.to_string(), "invalid LinkSys reference");
    }
    for name in ["", "a..b", "a/b", "A", "x:y", "bad_", "a."] {
        assert!(serde_json::from_value::<LinkSysRef>(serde_json::json!({"namespace":"default","name":name})).is_err());
    }
    let max_name = format!(
        "{}.{}.{}.{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    let reference = LinkSysRef {
        namespace: "n".repeat(63),
        name: max_name,
    };
    assert!(reference.validate().is_ok());
    let mut invalid = reference.clone();
    invalid.namespace.push('n');
    assert!(invalid.validate().is_err());
    let mut invalid = reference;
    invalid.name.push('d');
    assert!(invalid.validate().is_err());
    assert!(
        serde_json::from_value::<StringOutput>(serde_json::json!({"otlp":{"endpoint":"http://collector"}})).is_err()
    );
    assert!(serde_json::from_value::<StringOutput>(
        serde_json::json!({"otlp":{"linkSysRef":{"namespace":"default","name":"collector"},"headers":{}}})
    )
    .is_err());
}
