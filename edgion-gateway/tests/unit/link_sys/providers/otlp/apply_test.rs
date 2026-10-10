use std::collections::BTreeMap;

use edgion_resources::resources::common::outbound_tls::OutboundTlsLocal;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::otlp::{OtlpAuth, OtlpClientConfig};
use edgion_resources::resources::link_sys::{LinkSysLocalStatus, LinkSysSpec};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;

use super::*;
// Historical provider-field encoding; owning replay is covered separately.
fn fingerprint_otlp(owner: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    fingerprint_otlp_fields(owner, |_| {})
}

fn otlp_ls(endpoint: &str, timeout_ms: u64, rv: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some("fp".to_string()),
            name: Some("otlp".to_string()),
            resource_version: rv.map(str::to_string),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Otlp(OtlpClientConfig {
                endpoint: endpoint.to_string(),
                timeout_ms,
                auth: None,
                tls: None,
            }),
        },
        status: None,
    }
}

fn with_auth(mut ls: LinkSys, token: &[u8]) -> LinkSys {
    let SystemConfig::Otlp(config) = &mut ls.spec.config else {
        panic!("OTLP fixture");
    };
    let data = BTreeMap::from([("bearerToken".to_string(), ByteString(token.to_vec()))]);
    config.auth = Some(OtlpAuth {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "auth".to_string(),
                namespace: None,
            },
            secret: Some(Secret {
                metadata: kube::api::ObjectMeta {
                    name: Some("auth".into()),
                    namespace: Some("fp".into()),
                    ..Default::default()
                },
                data: Some(data),
                ..Default::default()
            }),
        },
    });
    ls
}

#[test]
fn endpoint_and_timeout_are_rebuild_fields() {
    let base = otlp_ls("https://a.example:4317", 10_000, None);
    let endpoint = otlp_ls("https://b.example:4317", 10_000, None);
    let timeout = otlp_ls("https://a.example:4317", 20_000, None);
    assert!(!fingerprint_otlp(&base)
        .unwrap()
        .equal_runtime(&fingerprint_otlp(&endpoint).unwrap()));
    assert!(!fingerprint_otlp(&base)
        .unwrap()
        .equal_runtime(&fingerprint_otlp(&timeout).unwrap()));
}

#[test]
fn bearer_and_tls_material_rebuild() {
    let base = otlp_ls("https://a.example:4317", 10_000, Some("1"));
    let token_a = with_auth(base.clone(), b"token-a");
    let token_b = with_auth(base.clone(), b"token-b");
    assert!(!fingerprint_otlp(&token_a)
        .unwrap()
        .equal_runtime(&fingerprint_otlp(&token_b).unwrap()));

    let mut ca_a = base.clone();
    let mut ca_b = base;
    for (ls, ca) in [(&mut ca_a, b"ca-a".as_slice()), (&mut ca_b, b"ca-b".as_slice())] {
        let SystemConfig::Otlp(config) = &mut ls.spec.config else {
            panic!("OTLP fixture");
        };
        config.tls = Some(TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                validation: Some(edgion_resources::resources::common::OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![edgion_resources::resources::common::CustomCACertificateRef {
                        group: None,
                        kind: "Secret".into(),
                        name: "ca".into(),
                        namespace: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: Some(vec![Secret {
                metadata: kube::core::ObjectMeta {
                    name: Some("ca".into()),
                    namespace: Some("fp".into()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(ca.to_vec()))])),
                ..Default::default()
            }]),
            resolved_client_certificate: None,
        });
    }
    assert!(!fingerprint_otlp(&ca_a)
        .unwrap()
        .equal_runtime(&fingerprint_otlp(&ca_b).unwrap()));
}

#[test]
fn resource_and_sync_versions_are_ignored() {
    let mut first = otlp_ls("https://a.example:4317", 10_000, Some("1"));
    let mut second = otlp_ls("https://a.example:4317", 10_000, Some("2"));
    first.metadata.annotations = Some(BTreeMap::from([(
        "edgion.io/sync-version".to_string(),
        "first".to_string(),
    )]));
    second.metadata.annotations = Some(BTreeMap::from([(
        "edgion.io/sync-version".to_string(),
        "second".to_string(),
    )]));
    second.status = Some(
        LinkSysLocalStatus {
            observed_generation: Some(7),
            ..Default::default()
        }
        .into(),
    );
    assert!(fingerprint_otlp(&first)
        .unwrap()
        .equal_runtime(&fingerprint_otlp(&second).unwrap()));
}

fn candidate(
    owner: LinkSys,
    defaults: Arc<edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot>,
) -> anyhow::Result<OtlpCandidate> {
    OtlpCandidate::from_prepared(
        edgion_common::link_sys_core::outbound_tls::PreparedOtlpTls::with_snapshot(Arc::new(owner), defaults)?,
    )
}

#[test]
fn owned_preparation_precedes_same_rv_and_zero_live_reuse_for_implicit_https() {
    use edgion_common::link_sys_core::outbound_tls::{test_util, GlobalOutboundTlsSnapshot};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let a = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    for endpoint in ["http://collector.example:4317", "https://collector.example:4317"] {
        for tls in [
            None,
            Some(serde_json::from_value(serde_json::json!({"enabled":false})).unwrap()),
        ] {
            let mut actual = otlp_ls(endpoint, 1_000, Some("42"));
            let SystemConfig::Otlp(config) = &mut actual.spec.config else {
                unreachable!()
            };
            config.tls = tls;
            let first = candidate(actual.clone(), a.clone()).unwrap();
            let second = candidate(actual.clone(), b.clone()).unwrap();
            assert_eq!(
                OtlpApply::fingerprint(&first)
                    .unwrap()
                    .rv_skip_allows_reuse(&OtlpApply::fingerprint(&second).unwrap()),
                endpoint.starts_with("http://")
            );
            let client = OtlpApply::build("fp/otlp", &first).unwrap();
            assert_eq!(OtlpApply::coverage(&client).live, 0);
            for field in ["name", "namespace", "empty-namespace", "invalid-namespace"] {
                let mut invalid = actual.clone();
                match field {
                    "name" => invalid.metadata.name = None,
                    "namespace" => invalid.metadata.namespace = None,
                    "empty-namespace" => invalid.metadata.namespace = Some("".into()),
                    _ => invalid.metadata.namespace = Some("Bad".into()),
                }
                assert!(candidate(invalid, a.clone()).is_err(), "{field}");
            }
            assert!(OtlpApply::build("fp/another", &first).is_err());
        }
    }
}

#[test]
fn actual_snapshot_capture_survives_publication_and_repeated_transport_construction() {
    use edgion_common::link_sys_core::outbound_tls::{
        outbound_global_default, set_outbound_global_default, GlobalOutboundTlsSnapshot,
    };
    const CHILD: &str = "EDGION_OTLP_OWNED_CAPTURE_CHILD";
    const TEST: &str = "link_sys::providers::otlp::apply::tests::actual_snapshot_capture_survives_publication_and_repeated_transport_construction";
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
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([])));
    let actual = Arc::new(otlp_ls("https://collector.example:4317", 1_000, Some("42")));
    let prepared = OtlpApply::prepare(actual.clone()).unwrap();
    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"}, "spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)])));
    for _ in 0..2 {
        let client = OtlpApply::build("fp/otlp", &prepared).unwrap();
        assert!(client
            .runtime_fingerprint()
            .equal_runtime(&OtlpApply::fingerprint(&prepared).unwrap()));
        client.assert_retained_transport_input(&prepared.tls, actual.as_ref());
    }
    assert!(OtlpApply::prepare(actual).is_err());
}

#[test]
fn proof_and_fingerprint_are_pure_and_cold_capability_failure_keeps_tls_audit() {
    use edgion_common::link_sys_core::outbound_tls::test_util;
    use edgion_resources::resources::common::OutboundTlsConfig;
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
    let subscriber = tracing_subscriber::registry().with(Audits(count.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    let defaults = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    let prepared = candidate(otlp_ls("https://collector.example:4317", 1_000, Some("42")), defaults).unwrap();
    for _ in 0..3 {
        let _ = OtlpApply::fingerprint(&prepared).unwrap();
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
    assert!(OtlpApply::build("fp/otlp", &prepared).is_err());
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "only cold resolution emits the unsupported verify audit"
    );
}

#[test]
fn auth_identity_is_checked_before_otlp_fingerprinting_even_at_equal_resource_version() {
    let valid = with_auth(
        otlp_ls("http://collector.example:4317", 1000, Some("42")),
        b"stable-token",
    );
    let expected = fingerprint_otlp(&valid).unwrap();
    assert_eq!(expected, fingerprint_otlp(&valid.clone()).unwrap());
    let mut invalid = valid.clone();
    let SystemConfig::Otlp(c) = &mut invalid.spec.config else {
        unreachable!()
    };
    c.auth.as_mut().unwrap().slot.secret.as_mut().unwrap().metadata.name = Some("other-secret".into());
    assert_eq!(invalid.metadata.resource_version, valid.metadata.resource_version);
    assert_eq!(
        fingerprint_otlp(&invalid).unwrap_err().to_string(),
        "OTLP bearer credential is invalid"
    );
    assert!(OtlpCandidate::new(Arc::new(invalid)).is_err());
    let mut missing_data = valid.clone();
    let SystemConfig::Otlp(c) = &mut missing_data.spec.config else {
        unreachable!()
    };
    c.auth.as_mut().unwrap().slot.secret.as_mut().unwrap().data = None;
    assert_eq!(
        fingerprint_otlp(&missing_data).unwrap_err().to_string(),
        "OTLP bearer credential is unresolved"
    );
}
