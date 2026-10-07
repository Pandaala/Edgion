use super::*;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
use edgion_resources::resources::link_sys::LinkSysSpec;

// Historical field-encoding tests do not establish owning authority.
fn fingerprint_kafka(owner: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    fingerprint_kafka_fields(owner, |_| Ok(()))
}

fn candidate(
    owner: LinkSys,
    defaults: Arc<edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot>,
) -> anyhow::Result<KafkaCandidate> {
    KafkaCandidate::from_prepared(PreparedLinkSysTls::with_snapshot(Arc::new(owner), defaults)?)
}

fn with_tls(owner: &mut LinkSys, tls: serde_json::Value) {
    let SystemConfig::Kafka(config) = &mut owner.spec.config else {
        unreachable!()
    };
    config.tls = Some(serde_json::from_value(tls).unwrap());
}

#[test]
fn actual_source_and_ordered_pairing_precede_same_rv_and_zero_live_planning() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    for tls in [
        None,
        Some(serde_json::json!({"enabled":false})),
        Some(serde_json::json!({"enabled":true,"verify":true})),
    ] {
        let mut actual = kafka_ls(vec!["kafka.example:9093"], None, Some("42"));
        if let Some(tls) = tls {
            with_tls(&mut actual, tls);
        }
        let valid = candidate(actual.clone(), defaults.clone()).unwrap();
        let zero_live = KafkaApply::build("fp/kafka", &valid).unwrap();
        assert_eq!(zero_live.connection_coverage().live, 0);
        assert!(zero_live
            .runtime_fingerprint()
            .rv_skip_allows_reuse(valid.fingerprint()));
        for field in ["name", "empty-namespace", "bad-namespace"] {
            let mut invalid = actual.clone();
            match field {
                "name" => invalid.metadata.name = None,
                "empty-namespace" => invalid.metadata.namespace = Some("".into()),
                _ => invalid.metadata.namespace = Some("Bad".into()),
            }
            assert!(candidate(invalid, defaults.clone()).is_err());
        }
        actual.metadata.namespace = None;
        assert!(candidate(actual, defaults.clone()).is_ok());
    }
    let mut actual = kafka_ls(vec!["kafka.example:9093"], None, Some("42"));
    let valid = serde_json::json!({"enabled":true,"verify":true,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"a"},{"kind":"Secret","name":"b"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"a","namespace":"fp"}},{"metadata":{"name":"b","namespace":"fp"}}],
        "clientCertificateRef":{"name":"client"},
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"fp"}}
    });
    with_tls(&mut actual, valid.clone());
    assert!(
        candidate(actual.clone(), defaults.clone()).is_ok(),
        "pure preparation does not parse PEM"
    );
    for field in [
        "group",
        "order",
        "extra-ca",
        "missing-client",
        "wrong-client",
        "extra-client",
        "marker",
        "bad-target",
    ] {
        let mut tls = valid.clone();
        match field {
            "group" => tls["validation"]["caCertificateRefs"][0]["group"] = "foreign.example".into(),
            "order" => tls["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
            "extra-ca" => tls["resolvedCaCertificates"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({})),
            "missing-client" => tls["resolvedClientCertificate"] = serde_json::Value::Null,
            "wrong-client" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = "other".into(),
            "extra-client" => tls["clientCertificateRef"] = serde_json::Value::Null,
            "marker" => {
                tls["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({"edgion.io/controller-ca-source":"ConfigMap"})
            }
            _ => tls["validation"]["caCertificateRefs"][0]["namespace"] = "".into(),
        }
        with_tls(&mut actual, tls.clone());
        assert!(candidate(actual.clone(), defaults.clone()).is_err(), "{field}");
        assert!(KafkaApply::prepare(Arc::new(actual.clone())).is_err(), "runner {field}");
        tls["enabled"] = false.into();
        with_tls(&mut actual, tls);
        assert!(candidate(actual.clone(), defaults.clone()).is_ok(), "inactive {field}");
    }
    let wrong = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"es"},"spec":{"type":"elasticsearch","config":{"endpoints":["https://es.example:9200"]}}
    }))
    .unwrap();
    assert!(candidate(wrong, defaults).is_err());
}

#[test]
fn consumed_inherited_axes_change_identity_but_native_and_local_overrides_mask_globals() {
    use edgion_common::link_sys_core::outbound_tls::test_util;
    use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsValidation};
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        validation: Some(OutboundTlsValidation {
            hostname: Some("changed.example".into()),
            well_known_ca_certificates: Some(
                edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System,
            ),
            ..Default::default()
        }),
        ..Default::default()
    });
    for enabled in [false, true] {
        let mut actual = kafka_ls(vec!["kafka.example:9093"], None, Some("42"));
        with_tls(&mut actual, serde_json::json!({"enabled":enabled}));
        let left = candidate(actual.clone(), a.clone()).unwrap();
        let right = candidate(actual.clone(), b.clone()).unwrap();
        assert_eq!(left.fingerprint().rv_skip_allows_reuse(right.fingerprint()), !enabled);
        with_tls(
            &mut actual,
            serde_json::json!({"enabled":enabled,"verify":true,"validation":{}}),
        );
        assert!(candidate(actual.clone(), a.clone())
            .unwrap()
            .fingerprint()
            .equal_runtime(candidate(actual, b.clone()).unwrap().fingerprint()));
    }
}

#[test]
fn actual_capture_survives_publication_and_connection_input_retains_owner_and_connector() {
    use edgion_common::link_sys_core::outbound_tls::{
        outbound_global_default, set_outbound_global_default, test_util, GlobalOutboundTlsSnapshot,
    };
    use edgion_resources::resources::common::OutboundTlsConfig;
    const CHILD: &str = "EDGION_KAFKA_OWNED_CAPTURE_CHILD";
    const TEST: &str = "link_sys::providers::kafka::apply::tests::actual_capture_survives_publication_and_connection_input_retains_owner_and_connector";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, TEST)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("1 passed; 0 failed"), "{stdout}");
        return;
    }
    struct Restore(Arc<GlobalOutboundTlsSnapshot>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_outbound_global_default(self.0.clone());
        }
    }
    let _restore = Restore(outbound_global_default());
    let _ = rustls::crypto::ring::default_provider().install_default();
    set_outbound_global_default(test_util::snapshot(OutboundTlsConfig::default()));
    let mut actual = kafka_ls(vec!["kafka.example:9093"], None, Some("42"));
    with_tls(&mut actual, serde_json::json!({"enabled":true}));
    let actual = Arc::new(actual);
    let captured = KafkaApply::prepare(actual.clone()).unwrap();
    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)])));
    let first = KafkaApply::build("fp/kafka", &captured).unwrap();
    let second = KafkaApply::build("fp/kafka", &captured).unwrap();
    assert!(first.runtime_fingerprint().equal_runtime(second.runtime_fingerprint()));
    first.assert_retained_connection_input(&captured.tls, actual.as_ref());
    second.assert_retained_connection_input(&captured.tls, actual.as_ref());
    assert!(KafkaApply::prepare(actual).is_err());
}

#[test]
fn prepare_and_fingerprint_do_not_resolve_material_or_emit_tls_audit() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    struct Audits(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> Layer<S> for Audits {
        fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            if event.metadata().target() == "edgion_common::link_sys_core::outbound_tls" {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(Audits(count.clone())));
    let mut actual = kafka_ls(vec!["kafka.example:9093"], None, Some("42"));
    with_tls(
        &mut actual,
        serde_json::json!({"enabled":true,"verify":true,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"fp"},"data":{"ca.crt":"bm90LXBlbQ=="}}]}),
    );
    let prepared = candidate(actual, Arc::new(GlobalOutboundTlsSnapshot::from_configs([]))).unwrap();
    for _ in 0..3 {
        let _ = KafkaApply::fingerprint(&prepared).unwrap();
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
    assert!(
        KafkaApply::build("fp/kafka", &prepared).is_err(),
        "PEM is parsed only during construction"
    );
}

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
