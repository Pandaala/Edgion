#[test]
fn malformed_effective_hostname_rejects_the_entire_attachment() {
    let mut route: TLSRoute = crate::test_current_status::from_value(serde_json::json!({
            "metadata": {"namespace": "apps", "name": "invalid-effective"},
            "spec": {"hostnames": ["*.example.com"], "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "gw"}],
                "resolvedListeners": [
                    {"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                        "gatewayName": "gw", "listenerName": "tls",
                        "hostnames": ["valid.example.com", "*.EXAMPLE.com"]}
                ]}
        }))
        .unwrap();
    assert!(TlsRouteUnit::from_route(Arc::new(route.clone()))
        .unwrap()
        .accepted_attachments()
        .is_empty());
    route.spec.resolved_listeners.as_mut().unwrap()[0].hostnames.pop();
    assert_eq!(
        TlsRouteUnit::from_route(Arc::new(route))
            .unwrap()
            .accepted_attachments()
            .len(),
        1
    );
}

#[test]
fn exact_attachment_parents_preserve_authored_owner_order() {
    let route = crate::test_current_status::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "ordered"},
        "spec": {"hostnames": ["shared.example.com"],
            "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "z"}, {"name": "a"}],
            "resolvedListeners": [
                {"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                    "gatewayName": "a", "listenerName": "tls", "hostnames": ["shared.example.com"]},
                {"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                    "gatewayName": "z", "listenerName": "tls", "hostnames": ["shared.example.com"]}
            ]}
    }))
    .unwrap();
    let unit = super::TlsRouteUnit::from_route(std::sync::Arc::new(route)).unwrap();
    let key = edgion_resources::ListenerKey::new("0.0.0.0:443".parse().unwrap(), "TLS");
    let parents: Vec<_> = unit
        .accepted_attachments()
        .iter()
        .filter(|accepted| {
            unit.route().spec.resolved_listeners.as_ref().unwrap()[accepted.schema_index].listener_key == key
        })
        .map(|accepted| &accepted.parent)
        .collect();
    assert_eq!(
        parents
            .iter()
            .map(|parent| parent.gateway_key.as_str())
            .collect::<Vec<_>>(),
        ["apps/z", "apps/a"]
    );
    assert!(parents
        .iter()
        .all(|parent| parent.section_name.as_deref() == Some("tls")));
}

use super::*;
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::tls_route::{TLSBackendRef, TLSRouteRule, TLSRouteSpec};

fn make_route(annotations: Option<std::collections::BTreeMap<String, String>>) -> Arc<TLSRoute> {
    Arc::new(crate::test_current_status::processed(TLSRoute {
        metadata: kube::api::ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("r".to_string()),
            annotations,
            ..Default::default()
        },
        spec: TLSRouteSpec {
            current_status: None,
            resolved_status_controller: Some("edgion.io/gateway-controller".into()),
            parent_refs: Some(vec![ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some("default".to_string()),
                name: "gw".to_string(),
                section_name: None,
                port: None,
            }]),
            hostnames: Some(vec!["test.com".to_string()]),
            rules: Some(vec![TLSRouteRule {
                backend_refs: Some(vec![TLSBackendRef {
                    name: "svc".to_string(),
                    namespace: Some("default".to_string()),
                    port: Some(8080),
                    weight: Some(1),
                    group: None,
                    kind: None,
                    ref_denied: None,
                }]),
            }]),
            resolved_listeners: None,
        },
        status: None,
    }))
}

fn annotated(pairs: &[(&str, &str)]) -> Arc<TLSRoute> {
    let mut a = std::collections::BTreeMap::new();
    for (k, v) in pairs {
        a.insert(k.to_string(), v.to_string());
    }
    make_route(Some(a))
}

#[test]
fn test_proxy_protocol_v2_resolved_on_unit() {
    let unit = TlsRouteUnit::from_route(annotated(&[("edgion.io/proxy-protocol", "v2")])).unwrap();
    assert_eq!(unit.proxy_protocol_version(), Some(2));
    assert!(unit.stream_plugin_store_key.is_none());
}

#[test]
fn test_proxy_protocol_invalid_resolves_none() {
    let unit = TlsRouteUnit::from_route(annotated(&[("edgion.io/proxy-protocol", "v1")])).unwrap();
    assert_eq!(unit.proxy_protocol_version(), None);
}

#[test]
fn test_stream_plugin_store_key_full_path() {
    let unit = TlsRouteUnit::from_route(annotated(&[(
        "edgion.io/edgion-stream-plugins",
        "sandbox/sandbox-stream-plugins",
    )]))
    .unwrap();
    assert_eq!(
        unit.stream_plugin_store_key,
        Some("sandbox/sandbox-stream-plugins".to_string())
    );
}

#[test]
fn test_combined_annotations() {
    let unit = TlsRouteUnit::from_route(annotated(&[
        ("edgion.io/proxy-protocol", "v2"),
        ("edgion.io/edgion-stream-plugins", "ns/plugins"),
    ]))
    .unwrap();
    assert_eq!(unit.proxy_protocol_version(), Some(2));
    assert_eq!(unit.stream_plugin_store_key, Some("ns/plugins".to_string()));
}

#[test]
fn test_no_annotations_defaults() {
    let unit = TlsRouteUnit::from_route(make_route(None)).unwrap();
    assert_eq!(unit.proxy_protocol_version(), None);
    assert!(unit.stream_plugin_store_key.is_none());
    assert_eq!(unit.max_connect_retries(), DEFAULT_TLS_CONNECT_RETRIES);
    assert!(unit.tcp_keepalive().is_none());
}

#[test]
fn test_keepalive_annotation_resolved_on_unit() {
    let unit = TlsRouteUnit::from_route(annotated(&[
        ("edgion.io/tcp-keepalive-time", "120"),
        ("edgion.io/tcp-keepalive-interval", "20"),
        ("edgion.io/tcp-keepalive-probes", "4"),
    ]))
    .unwrap();
    let ka = unit.tcp_keepalive().expect("keepalive enabled");
    assert_eq!(ka.idle, std::time::Duration::from_secs(120));
    assert_eq!(ka.interval, std::time::Duration::from_secs(20));
    assert_eq!(ka.count, 4);
}

#[test]
fn test_backend_finder_built_from_first_rule() {
    let unit = TlsRouteUnit::from_route(make_route(None)).unwrap();
    assert!(unit.backend_generation().select().is_ok());
}

#[test]
fn denied_core_service_ref_remains_selectable() {
    let mut route = make_route(None);
    {
        let route = Arc::make_mut(&mut route);
        let backend = route
            .spec
            .rules
            .as_mut()
            .and_then(|rules| rules.first_mut())
            .and_then(|rule| rule.backend_refs.as_mut())
            .and_then(|refs| refs.first_mut())
            .expect("fixture has one backend ref");
        backend.ref_denied = Some(RefDenied {
            target_namespace: "other".to_string(),
            target_name: "svc".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
    }
    let unit = TlsRouteUnit::from_route(route).unwrap();
    let selected = unit
        .backend_generation()
        .select()
        .expect("denied core Service stays selectable");
    assert!(unit.route().spec.rules.as_ref().unwrap()[0]
        .backend_refs
        .as_ref()
        .unwrap()[selected.slot()]
    .ref_denied
    .is_some());
}

fn tls_backend(name: &str, group: Option<&str>, kind: Option<&str>, denied: bool) -> TLSBackendRef {
    TLSBackendRef {
        name: name.to_string(),
        namespace: Some("default".to_string()),
        port: Some(8080),
        weight: Some(1),
        group: group.map(str::to_string),
        kind: kind.map(str::to_string),
        ref_denied: denied.then(|| RefDenied {
            target_namespace: "other".to_string(),
            target_name: name.to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }),
    }
}

fn mixed_weight_route() -> Arc<TLSRoute> {
    let mut route = make_route(None);
    {
        let route = Arc::make_mut(&mut route);
        let refs = route
            .spec
            .rules
            .as_mut()
            .and_then(|rules| rules.first_mut())
            .and_then(|rule| rule.backend_refs.as_mut())
            .expect("fixture has backend refs");
        *refs = vec![
            tls_backend("allowed", None, None, false),
            tls_backend("denied", None, None, true),
            tls_backend("invalid", Some("apps"), Some("Deployment"), false),
        ];
    }
    route
}

#[test]
fn mixed_denied_and_unsupported_keep_declared_weight() {
    use crate::routes::common::{classify_l4_selected_backend, L4SelectedBackend};
    let unit = TlsRouteUnit::from_route(mixed_weight_route()).unwrap();
    let mut allowed = 0;
    let mut denied = 0;
    let mut invalid = 0;
    for _ in 0..3 {
        let selected = unit.backend_generation().select().expect("three equal-weight slots");
        let outcome = classify_l4_selected_backend(selected);
        match unit.route().spec.rules.as_ref().unwrap()[0]
            .backend_refs
            .as_ref()
            .unwrap()[selected.slot()]
        .name
        .as_str()
        {
            "allowed" => {
                allowed += 1;
                assert_eq!(outcome, L4SelectedBackend::Proceed);
            }
            "denied" => {
                denied += 1;
                assert_eq!(outcome, L4SelectedBackend::Denied);
            }
            "invalid" => {
                invalid += 1;
                assert_eq!(outcome, L4SelectedBackend::UnsupportedKind);
            }
            other => panic!("unexpected backend {other}"),
        }
    }
    assert_eq!((allowed, denied, invalid), (1, 1, 1));
}

impl TlsRouteUnit {
    pub(crate) fn backend_generation(&self) -> &Arc<CompiledL4Backends> {
        &self.backend_finder
    }
}
