use super::*;
use std::sync::atomic::Ordering;

use crate::plugins::http::common::outbound::target::ResolvedTarget;
use edgion_resources::resources::common::WebhookTarget;
use edgion_resources::resources::common::{
    ExternalHttpConnection, MethodSpec, OutboundRequestSpec, PathSpec, WebhookServiceRef,
};

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
    WebhookLinkClient::prepare("ns/wh", config, Some("ns"), RuntimeFingerprint::unspecified())
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
