use super::*;
use std::sync::atomic::Ordering;

use crate::plugins::http::common::outbound::target::ResolvedTarget;
use edgion_resources::resources::common::WebhookTarget;
use edgion_resources::resources::common::{
    ExternalHttpConnection, MethodSpec, OutboundRequestSpec, PathSpec, WebhookServiceRef,
};
use edgion_resources::resources::link_sys::{webhook::WebhookServiceConfig, LinkSysSpec};

fn config() -> WebhookServiceConfig {
    WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    name: Some("example-svc".into()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn prepare(config: WebhookServiceConfig) -> anyhow::Result<WebhookLinkClient> {
    WebhookCandidate::with_snapshot(owner(config), Arc::new(GlobalOutboundTlsSnapshot::from_configs([])))
        .unwrap()
        .prepare("ns/wh")
}

fn owner(config: WebhookServiceConfig) -> Arc<LinkSys> {
    let mut owner = LinkSys::new(
        "wh",
        LinkSysSpec {
            config: SystemConfig::Webhook(config),
            current_status: None,
            unknown_fields: Vec::new(),
        },
    );
    owner.metadata.namespace = Some("ns".into());
    Arc::new(owner)
}

fn candidate(owner: Arc<LinkSys>) -> WebhookCandidate {
    WebhookCandidate::with_snapshot(owner, Arc::new(GlobalOutboundTlsSnapshot::from_configs([]))).unwrap()
}

#[test]
fn owning_source_proof_precedes_both_reuse_shortcuts_without_tls() {
    let mut valid = (*owner(config())).clone();
    valid.metadata.resource_version = Some("42".into());
    let valid = Arc::new(valid);
    let ready = candidate(Arc::clone(&valid)).prepare("ns/wh").unwrap();
    assert!(candidate(Arc::clone(&valid)).can_reuse(&ready, false));
    assert!(!candidate(Arc::clone(&valid)).can_reuse(&ready, true));
    for field in ["name", "empty-namespace", "bad-namespace"] {
        let mut bad = (*valid).clone();
        match field {
            "name" => bad.metadata.name = None,
            "empty-namespace" => bad.metadata.namespace = Some("".into()),
            _ => bad.metadata.namespace = Some("Bad".into()),
        }
        let incoming = candidate(Arc::new(bad));
        assert!(!incoming.can_reuse(&ready, false), "{field}");
        assert!(incoming.prepare("ns/wh").is_err(), "{field}");
    }
    let mut wrong = (*valid).clone();
    wrong.spec.config = SystemConfig::Unsupported;
    assert!(WebhookCandidate::from_link_sys(Arc::new(wrong)).is_none());
    let binding = PreparedLinkSysReferences::from_link_sys(&valid);
    let binding = binding.bind(&valid).unwrap();
    assert!(ManagedOutboundClient::from_webhook(&binding, &GlobalOutboundTlsSnapshot::from_configs([]), "x").is_ok());
    let wrong: LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"dns"},"spec":{"type":"httpDns","config":{"preset":"google"}}
    }))
    .unwrap();
    let references = PreparedLinkSysReferences::from_link_sys(&wrong);
    let binding = references.bind(&wrong).unwrap();
    assert!(ManagedOutboundClient::from_webhook(&binding, &GlobalOutboundTlsSnapshot::from_configs([]), "x").is_err());
}

#[test]
fn omitted_owner_namespace_defaults_service_target_and_explicit_empty_fails() {
    let mut actual = (*owner(config())).clone();
    actual.metadata.namespace = None;
    let ready = candidate(Arc::new(actual.clone())).prepare("default/wh").unwrap();
    assert!(matches!(&ready.client.resolved_target,
        ResolvedTarget::Service { service_key, .. } if service_key == "default/example-svc"));
    actual.metadata.namespace = Some("".into());
    assert!(candidate(Arc::new(actual)).prepare("default/wh").is_err());
}

#[test]
fn disabled_tls_with_dormant_invalid_material_reuses_absent_tls_across_versions() {
    let mut actual = (*owner(config())).clone();
    actual.metadata.resource_version = Some("1".into());
    let ready = candidate(Arc::new(actual.clone())).prepare("ns/wh").unwrap();
    actual.metadata.resource_version = Some("2".into());
    let SystemConfig::Webhook(config) = &mut actual.spec.config else {
        unreachable!()
    };
    config.conn.tls = Some(
        serde_json::from_value(serde_json::json!({
            "enabled":false,
            "validation":{"caCertificateRefs":[{"group":"foreign.example","kind":"Secret","name":"bad"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"wrong","namespace":"wrong"}}]
        }))
        .unwrap(),
    );
    let incoming = candidate(Arc::new(actual));
    assert!(incoming.can_reuse(&ready, false));
    assert!(!incoming.can_reuse(&ready, true));
}

fn active_ca_config() -> WebhookServiceConfig {
    let mut config = config();
    config.conn.tls = Some(
        serde_json::from_value(serde_json::json!({
            "enabled":true,"verify":false,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"ns"},"data":{"ca.crt":"Y2VydGlmaWNhdGU="}}]
        }))
        .unwrap(),
    );
    config
}

#[test]
fn active_reference_material_proof_precedes_same_rv_and_verify_false() {
    let mut valid = (*owner(active_ca_config())).clone();
    valid.metadata.resource_version = Some("42".into());
    let valid = Arc::new(valid);
    let ready = candidate(Arc::clone(&valid)).prepare("ns/wh").unwrap();
    for field in ["foreign-group", "missing", "namespace", "name", "extra", "provenance"] {
        let mut bad = (*valid).clone();
        let SystemConfig::Webhook(config) = &mut bad.spec.config else {
            unreachable!()
        };
        let tls = config.conn.tls.as_mut().unwrap();
        match field {
            "foreign-group" => {
                tls.outbound
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs
                    .as_mut()
                    .unwrap()[0]
                    .group = Some("foreign.example".into())
            }
            "missing" => tls.resolved_ca_certificates = None,
            "namespace" => tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into()),
            "name" => tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some("other".into()),
            "extra" => tls.resolved_ca_certificates.as_mut().unwrap().push(Default::default()),
            _ => {
                tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.annotations =
                    Some(std::collections::BTreeMap::from([(
                        edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE.into(),
                        "ConfigMap".into(),
                    )]))
            }
        }
        let incoming = candidate(Arc::new(bad.clone()));
        assert!(!incoming.can_reuse(&ready, false), "{field}");
        assert!(incoming.prepare("ns/wh").is_err(), "{field}");
        let SystemConfig::Webhook(config) = &mut bad.spec.config else {
            unreachable!()
        };
        config.conn.tls.as_mut().unwrap().enabled = false;
        assert!(candidate(Arc::new(bad)).prepare("ns/wh").is_ok(), "inactive {field}");
    }
    assert!(candidate(valid).can_reuse(&ready, false));
}

#[test]
fn captured_inheritance_masks_overrides_and_unmasked_changes_prevent_same_rv_reuse() {
    use edgion_common::link_sys_core::outbound_tls::test_util;
    use edgion_resources::resources::common::OutboundTlsConfig;
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    let mut config = config();
    config.conn.tls = Some(serde_json::from_value(serde_json::json!({"enabled":true})).unwrap());
    let mut actual = (*owner(config.clone())).clone();
    actual.metadata.resource_version = Some("42".into());
    let actual = Arc::new(actual);
    let captured = WebhookCandidate::with_snapshot(Arc::clone(&actual), Arc::clone(&a)).unwrap();
    let ready = captured.prepare("ns/wh").unwrap();
    assert!(WebhookCandidate::with_snapshot(Arc::clone(&actual), Arc::clone(&a))
        .unwrap()
        .can_reuse(&ready, false));
    assert!(!WebhookCandidate::with_snapshot(Arc::clone(&actual), Arc::clone(&b))
        .unwrap()
        .can_reuse(&ready, false));
    config.conn.tls.as_mut().unwrap().outbound.verify = Some(false);
    let actual = owner(config);
    let ready = WebhookCandidate::with_snapshot(Arc::clone(&actual), Arc::clone(&a))
        .unwrap()
        .prepare("ns/wh")
        .unwrap();
    assert!(WebhookCandidate::with_snapshot(Arc::clone(&actual), b)
        .unwrap()
        .can_reuse(&ready, false));
    assert!(Arc::ptr_eq(&ready.managed_client(), &ready.managed_client()));
}

#[test]
fn candidate_comparison_does_not_parse_material_or_emit_tls_audit() {
    use std::sync::atomic::AtomicUsize;
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    struct AuditCounter(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> Layer<S> for AuditCounter {
        fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            if event.metadata().target() == "edgion_common::link_sys_core::outbound_tls"
                && *event.metadata().level() == tracing::Level::WARN
            {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(AuditCounter(Arc::clone(&count)));
    let _guard = tracing::subscriber::set_default(subscriber);
    // Deliberately non-PEM CA bytes: verify=false proves the complete pair but
    // never parses the unused CA body, including during equality comparisons.
    let actual = owner(active_ca_config());
    let ready = candidate(Arc::clone(&actual)).prepare("ns/wh").unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    for _ in 0..3 {
        assert!(candidate(Arc::clone(&actual)).can_reuse(&ready, false));
    }
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn captured_actual_snapshot_survives_global_publication() {
    use edgion_common::link_sys_core::outbound_tls::{set_outbound_global_default, test_util};
    use edgion_resources::resources::common::OutboundTlsConfig;
    const CHILD_ENV: &str = "EDGION_WEBHOOK_CAPTURE_TLS_CHILD";
    const TEST_NAME: &str =
        "link_sys::providers::webhook::client::tests::captured_actual_snapshot_survives_global_publication";
    // Other Gateway TLS tests also publish global defaults. Run this deliberate
    // publication interleaving in its own process, retaining unwind restoration.
    if std::env::var(CHILD_ENV).as_deref() != Ok(TEST_NAME) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, TEST_NAME)
            .output()
            .expect("isolated Webhook TLS capture test must start");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "the child must execute its exact test: {stdout}"
        );
        return;
    }
    struct Restore(Arc<GlobalOutboundTlsSnapshot>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_outbound_global_default(Arc::clone(&self.0));
        }
    }
    let _restore = Restore(outbound_global_default());
    let a = test_util::snapshot(OutboundTlsConfig::default());
    set_outbound_global_default(Arc::clone(&a));
    let mut config = config();
    config.conn.tls = Some(serde_json::from_value(serde_json::json!({"enabled":true})).unwrap());
    let actual = owner(config);
    let captured = WebhookCandidate::from_link_sys(Arc::clone(&actual)).unwrap();
    let invalid: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"invalid","namespace":"apps"},"spec":{}
    }))
    .unwrap();
    let b = Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(invalid)]));
    set_outbound_global_default(Arc::clone(&b));
    let ready = captured
        .prepare("ns/wh")
        .expect("captured usable A must construct after invalid B is published");
    let fresh = WebhookCandidate::from_link_sys(Arc::clone(&actual)).unwrap();
    assert!(!fresh.can_reuse(&ready, false));
    assert!(fresh.prepare("ns/wh").is_err(), "a new capture must consume invalid B");
    assert!(WebhookCandidate::with_snapshot(actual, a)
        .unwrap()
        .can_reuse(&ready, false));
}

#[test]
fn managed_client_preserves_identity() {
    let owner = prepare(config()).unwrap();
    assert!(Arc::ptr_eq(&owner.managed_client(), &owner.managed_client()));
    assert!(owner.managed_client().health.healthy.load(Ordering::Relaxed));
}

#[test]
fn service_target_defaults_kind_and_namespace() {
    let owner = prepare(config()).unwrap();
    match &owner.client.resolved_target {
        ResolvedTarget::Service {
            service_key,
            port,
            scheme,
            ..
        } => {
            assert_eq!(service_key, "ns/example-svc");
            assert_eq!(*port, Some(8080));
            assert_eq!(*scheme, "http");
        }
        other => panic!("expected Service target, got {other:?}"),
    }
}

#[test]
fn cluster_ip_target_preserves_explicit_namespace_or_inherits_linksys_namespace() {
    for (namespace, expected) in [(None, "ns"), (Some("prod"), "prod")] {
        let mut cfg = config();
        cfg.conn.target.service.kind = Some("ServiceClusterIp".into());
        cfg.conn.target.service.namespace = namespace.map(str::to_string);
        let owner = prepare(cfg).unwrap();
        match &owner.client.resolved_target {
            ResolvedTarget::Static(url) => {
                assert_eq!(url, &format!("http://example-svc.{expected}.svc.cluster.local:8080"));
            }
            other => panic!("expected static target, got {other:?}"),
        }
    }
}

#[test]
fn url_target_does_not_inherit_namespace() {
    let mut cfg = config();
    cfg.conn.target = WebhookTarget {
        url: Some("https://api.example.com".into()),
        ..Default::default()
    };
    let owner = prepare(cfg).unwrap();
    assert!(matches!(&owner.client.resolved_target,
            ResolvedTarget::Static(url) if url == "https://api.example.com"));
}

#[test]
fn unsupported_service_kind_is_rejected() {
    let mut cfg = config();
    cfg.conn.target.service.kind = Some("ServiceImport".into());
    assert!(prepare(cfg).is_err());
}

#[test]
fn invalid_request_template_is_rejected_before_publication() {
    let mut cfg = config();
    cfg.request.path = Some(PathSpec {
        template: "/api/${ctx:missing_close".into(),
        allow_override: false,
    });
    let error = prepare(cfg).err().expect("unclosed template must fail").to_string();
    assert!(error.contains("unclosed") || error.contains("compile"), "{error}");
}

#[test]
fn replacement_has_fresh_health_and_carries_resolved_secrets() {
    let first = prepare(config()).unwrap();
    first.client.health.healthy.store(false, Ordering::Relaxed);
    first.client.health.passive_failures.store(5, Ordering::Relaxed);
    let mut cfg = config();
    cfg.request = OutboundRequestSpec {
        method: Some(MethodSpec {
            template: "POST".into(),
        }),
        ..Default::default()
    };
    cfg.resolved_secrets = Some(
        serde_json::from_value(serde_json::json!({
            "ns/token:value": "test-token"
        }))
        .unwrap(),
    );
    let second = prepare(cfg).unwrap();
    assert!(!Arc::ptr_eq(&first.client, &second.client));
    assert!(second.client.health.healthy.load(Ordering::Relaxed));
    assert_eq!(second.client.health.passive_failures.load(Ordering::Relaxed), 0);
    assert_eq!(
        second.client.resolved_secrets.as_ref().unwrap().0["ns/token:value"],
        "test-token"
    );
}

#[test]
fn disabled_tls_ignores_material_and_preserves_target_scheme() {
    use serde_json::json;

    for url in [None, Some("http://example.com"), Some("https://example.com")] {
        for injected in [false, true] {
            let mut cfg = config();
            if let Some(url) = url {
                cfg.conn.target = WebhookTarget {
                    url: Some(url.into()),
                    ..Default::default()
                };
            }
            let mut tls = json!({
                "enabled": false,
                "verify": true,
                "validation": {"caCertificateRefs": [{"kind": "Secret", "name": "missing-ca"}]},
                "clientCertificateRef": {"name": "missing-client"}
            });
            if injected {
                tls["resolvedCaCertificates"] = json!([{}]);
                tls["resolvedClientCertificate"] = json!({});
            }
            for enabled in [false, true, false] {
                tls["enabled"] = json!(enabled);
                cfg.conn.tls = Some(serde_json::from_value(tls.clone()).unwrap());
                let result = prepare(cfg.clone());
                if enabled {
                    assert!(result.is_err(), "enabled invalid TLS must not prepare: {url:?}");
                    continue;
                }
                let owner = result.expect("disabled TLS must not block preparation");
                match (&owner.client.resolved_target, url) {
                    (ResolvedTarget::Service { scheme, .. }, None) => assert_eq!(*scheme, "http"),
                    (ResolvedTarget::Static(actual), Some(expected)) => assert_eq!(actual, expected),
                    (actual, expected) => panic!("unexpected target {actual:?} for {expected:?}"),
                }
            }
        }
    }
}

/// EndpointSlice churn does not replace the owned client or its health state.
///
/// Regression test: the `ResolvedTarget::Service` variant reads the
/// global EpSliceStore at call time. Endpoint add/remove/re-add must never
/// rebuild the `ManagedOutboundClient` or reset its health atomics.
#[tokio::test]
async fn endpoint_churn_preserves_health_state() {
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap, HashSet};

    let mut cfg = config();
    cfg.conn.target.service.name = Some("ep-churn-svc".into());
    cfg.conn.target.service.namespace = Some("default".into());
    let owner = prepare(cfg).unwrap();

    // Record the Arc identity and degrade the client health.
    let client = owner.managed_client();
    let ptr_before = Arc::as_ptr(&client);
    client.health.healthy.store(false, Ordering::Relaxed);
    client.health.passive_failures.store(3, Ordering::Relaxed);

    // Simulate endpoint churn by updating the EpSliceStore directly.
    let store = get_roundrobin_store();
    let mut labels = BTreeMap::new();
    labels.insert(SERVICE_NAME.to_string(), "ep-churn-svc".to_string());
    let ep_slice = EndpointSlice {
        metadata: ObjectMeta {
            name: Some("ep-churn-slice".to_string()),
            namespace: Some("default".to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        endpoints: vec![Endpoint {
            addresses: vec!["10.0.99.1".to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ports: Some(vec![EndpointPort {
            port: Some(8080),
            ..Default::default()
        }]),
    };
    let slice_key = "default/ep-churn-svc-slice";
    store.update_data_only(
        HashMap::from([(slice_key.to_string(), ep_slice)]),
        HashMap::new(),
        &HashSet::new(),
    );

    // The owner still exposes the same client after endpoint churn.
    let client_after = owner.managed_client();
    assert_eq!(
        Arc::as_ptr(&client_after),
        ptr_before,
        "endpoint churn must not replace the client Arc"
    );
    assert!(
        !client_after.health.healthy.load(Ordering::Relaxed),
        "healthy must remain false after endpoint churn"
    );
    assert_eq!(
        client_after.health.passive_failures.load(Ordering::Relaxed),
        3,
        "passive_failures must remain 3 after endpoint churn"
    );

    // Cleanup.
    store.update_data_only(HashMap::new(), HashMap::new(), &HashSet::from([slice_key.to_string()]));
}

#[tokio::test]
async fn retirement_and_drop_stop_probes_even_with_retained_call_handles() {
    for explicit_retire in [false, true] {
        let mut managed = ManagedOutboundClient::test_with_static_target("https://example.com");
        // A pending synthetic probe proves ownership without any network IO.
        managed.health_task = Some(tokio::spawn(std::future::pending()));
        let task = managed.health_task.as_ref().unwrap().abort_handle();
        let managed = Arc::new(managed);
        let owner = WebhookLinkClient::from_managed(managed.clone());
        if explicit_retire {
            owner.retire();
            owner.retire();
        } else {
            drop(owner);
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retirement must abort the probe despite a retained call handle");
        assert!(managed.health.healthy.load(Ordering::Relaxed));
        // Keep the client alive until after observing the abort, so its own
        // Drop cannot be the mechanism making this assertion pass.
        drop(managed);
    }
}
