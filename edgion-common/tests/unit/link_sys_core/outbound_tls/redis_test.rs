use super::*;
use crate::link_sys_core::outbound_tls::test_util;
use edgion_resources::resources::common::OutboundTlsConfig;

fn owner(endpoint: &str, tls: Option<serde_json::Value>) -> Arc<LinkSys> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "metadata":{"name":"redis","namespace":"apps","resourceVersion":"42"},
            "spec":{"type":"redis","config":{"endpoints":[endpoint],"tls":tls}}
        }))
        .unwrap(),
    )
}

fn empty() -> Arc<GlobalOutboundTlsSnapshot> {
    Arc::new(GlobalOutboundTlsSnapshot::from_configs([]))
}

fn hash(prepared: &PreparedRedisTls) -> [u8; 32] {
    let mut hasher = FingerprintHasher::new();
    prepared.hash_inheritance(&mut hasher);
    hasher.finish()
}

#[test]
fn actual_source_is_required_without_local_or_transport_tls() {
    let valid = owner("redis://redis.example:6379", None);
    assert!(!PreparedRedisTls::with_snapshot(Arc::clone(&valid), empty())
        .unwrap()
        .uses_tls());
    for field in ["name", "namespace", "empty-namespace"] {
        let mut bad = (*valid).clone();
        match field {
            "name" => bad.metadata.name = None,
            "namespace" => bad.metadata.namespace = Some("Bad".into()),
            _ => bad.metadata.namespace = Some("".into()),
        }
        assert!(
            PreparedRedisTls::with_snapshot(Arc::new(bad), empty()).is_err(),
            "{field}"
        );
    }
    let mut defaulted = (*valid).clone();
    defaulted.metadata.namespace = None;
    assert!(PreparedRedisTls::with_snapshot(Arc::new(defaulted), empty()).is_ok());
    let wrong: LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"dns"},"spec":{"type":"httpDns","config":{"preset":"google"}}
    }))
    .unwrap();
    assert!(PreparedRedisTls::with_snapshot(Arc::new(wrong), empty()).is_err());
}

#[test]
fn whole_active_pairs_are_checked_in_author_order_and_disabled_refs_are_inert() {
    let active = serde_json::json!({"enabled":true,"verify":false,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"first"},{"group":"core","kind":"Secret","name":"second"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"first","namespace":"apps"}},{"metadata":{"name":"second","namespace":"apps"}}]
    });
    assert!(
        PreparedRedisTls::with_snapshot(owner("redis://redis.example:6379", Some(active.clone())), empty()).is_ok()
    );
    for field in ["foreign-group", "missing", "extra", "order", "namespace", "marker"] {
        let mut bad = active.clone();
        match field {
            "foreign-group" => bad["validation"]["caCertificateRefs"][0]["group"] = "foreign.example".into(),
            "missing" => bad["resolvedCaCertificates"] = serde_json::Value::Null,
            "extra" => bad["resolvedCaCertificates"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({})),
            "order" => bad["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
            "namespace" => bad["resolvedCaCertificates"][0]["metadata"]["namespace"] = "other".into(),
            _ => {
                bad["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({"edgion.io/controller-ca-source":"ConfigMap"})
            }
        }
        assert!(
            PreparedRedisTls::with_snapshot(owner("redis://redis.example:6379", Some(bad.clone())), empty()).is_err(),
            "{field}"
        );
        bad["enabled"] = false.into();
        let prepared =
            PreparedRedisTls::with_snapshot(owner("rediss://redis.example:6380", Some(bad)), empty()).unwrap();
        assert!(prepared.uses_tls());
        assert!(prepared.redis_tls_material().unwrap().is_some());
    }
}

#[test]
fn implicit_tls_consumes_inheritance_and_plaintext_consumes_none() {
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    let implicit = owner("rediss://redis.example:6380", None);
    let prepared = PreparedRedisTls::with_snapshot(Arc::clone(&implicit), Arc::clone(&a)).unwrap();
    assert!(prepared.uses_tls());
    assert_ne!(
        hash(&prepared),
        hash(&PreparedRedisTls::with_snapshot(implicit, Arc::clone(&b)).unwrap())
    );
    let plain = owner("redis://redis.example:6379", None);
    assert_eq!(
        hash(&PreparedRedisTls::with_snapshot(Arc::clone(&plain), a).unwrap()),
        hash(&PreparedRedisTls::with_snapshot(plain, b).unwrap())
    );
    let invalid: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"global","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    let bad = Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(invalid)]));
    assert!(PreparedRedisTls::with_snapshot(owner("rediss://redis.example:6380", None), Arc::clone(&bad)).is_err());
    assert!(PreparedRedisTls::with_snapshot(
        owner(
            "rediss://redis.example:6380",
            Some(serde_json::json!({"enabled":true,"verify":true}))
        ),
        bad
    )
    .is_ok());
}

#[test]
fn captured_actual_global_generation_is_used_for_every_construction_attempt() {
    use super::super::{outbound_global_default, set_outbound_global_default};
    const CHILD: &str = "EDGION_REDIS_OWNED_CAPTURE_CHILD";
    const TEST: &str = "link_sys_core::outbound_tls::redis::tests::captured_actual_global_generation_is_used_for_every_construction_attempt";
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
            set_outbound_global_default(Arc::clone(&self.0));
        }
    }
    let _restore = Restore(outbound_global_default());
    let usable = test_util::snapshot(OutboundTlsConfig::default());
    set_outbound_global_default(Arc::clone(&usable));
    let actual = owner("rediss://redis.example:6380", None);
    let captured = PreparedRedisTls::from_link_sys(Arc::clone(&actual)).unwrap();
    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)])));
    assert!(captured.redis_tls_material().unwrap().is_some());
    assert!(captured.redis_tls_material().unwrap().is_some());
    assert!(PreparedRedisTls::from_link_sys(Arc::clone(&actual)).is_err());
    assert_eq!(
        hash(&captured),
        hash(&PreparedRedisTls::with_snapshot(actual, usable).unwrap())
    );
}

#[test]
fn comparison_is_pure_and_transport_construction_keeps_provider_rejections() {
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
    let subscriber = tracing_subscriber::registry().with(Audits(Arc::clone(&count)));
    let _guard = tracing::subscriber::set_default(subscriber);
    let prepared = PreparedRedisTls::with_snapshot(owner("rediss://redis.example:6380", Some(serde_json::json!({
        "enabled":true,"verify":false,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"apps"},"data":{"ca.crt":"bm90LXBlbQ=="}}]
    }))), empty()).unwrap();
    for _ in 0..3 {
        let _ = hash(&prepared);
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
    assert!(prepared
        .redis_tls_material()
        .err()
        .unwrap()
        .to_string()
        .contains("Redis TLS configuration error"));
    for policy in [
        serde_json::json!({"hostname":"redis.example"}),
        serde_json::json!({"subjectAltNames":[{"type":"Hostname","hostname":"redis.example"}]}),
    ] {
        let prepared = PreparedRedisTls::with_snapshot(
            owner(
                "rediss://redis.example:6380",
                Some(serde_json::json!({"enabled":true,"validation":policy})),
            ),
            empty(),
        )
        .unwrap();
        assert!(prepared.redis_tls_material().is_err());
    }
}

#[test]
fn redis_pem_material_distinguishes_plaintext_from_default_trust() {
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let plain = PreparedRedisTls::with_snapshot(owner("redis://redis.example:6379", None), defaults.clone()).unwrap();
    assert!(plain.redis_tls_material().unwrap().is_none());
    let tls = PreparedRedisTls::with_snapshot(owner("rediss://redis.example:6380", None), defaults).unwrap();
    let material = tls.redis_tls_material().unwrap().unwrap();
    // Default trust is the explicit process System bundle, never redis-rs's own store.
    assert_eq!(material.root_pem.as_slice(), crate::system_trust::system_trust().pem());
    assert!(material.client_identity.is_none());
}
