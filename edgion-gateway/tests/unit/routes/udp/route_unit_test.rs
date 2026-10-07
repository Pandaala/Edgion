use super::*;
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::udp_route::{UDPBackendRef, UDPRouteRule, UDPRouteSpec};

fn make_route(annotations: Option<std::collections::BTreeMap<String, String>>) -> Arc<UDPRoute> {
    Arc::new(crate::test_current_status::processed(UDPRoute {
        metadata: kube::api::ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("r".to_string()),
            annotations,
            ..Default::default()
        },
        spec: UDPRouteSpec {
            current_status: None,
            resolved_status_controller: Some("edgion.io/gateway-controller".into()),
            parent_refs: Some(vec![ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some("default".to_string()),
                name: "gw".to_string(),
                section_name: Some("udp".to_string()),
                port: None,
            }]),
            rules: Some(vec![UDPRouteRule {
                backend_refs: Some(vec![UDPBackendRef {
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
            resolved_listener_attachments: None,
        },
        status: None,
    }))
}

#[test]
fn test_stream_plugin_store_key_resolved_on_unit() {
    let mut a = std::collections::BTreeMap::new();
    a.insert("edgion.io/edgion-stream-plugins".to_string(), "ns/plugins".to_string());
    let unit = UdpRouteUnit::from_route(make_route(Some(a))).unwrap();
    assert_eq!(unit.stream_plugin_store_key, Some("ns/plugins".to_string()));
}

#[test]
fn test_no_annotation_leaves_store_key_none() {
    let unit = UdpRouteUnit::from_route(make_route(None)).unwrap();
    assert!(unit.stream_plugin_store_key.is_none());
}

#[test]
fn test_backend_finder_built_from_first_rule() {
    let unit = UdpRouteUnit::from_route(make_route(None)).unwrap();
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
    let unit = UdpRouteUnit::from_route(route).unwrap();
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

fn udp_backend(name: &str, group: Option<&str>, kind: Option<&str>, denied: bool) -> UDPBackendRef {
    UDPBackendRef {
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

fn mixed_weight_route() -> Arc<UDPRoute> {
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
            udp_backend("allowed", None, None, false),
            udp_backend("denied", None, None, true),
            udp_backend("invalid", Some("apps"), Some("Deployment"), false),
        ];
    }
    route
}

#[test]
fn mixed_denied_and_unsupported_keep_declared_weight() {
    use crate::routes::common::{classify_l4_selected_backend, L4SelectedBackend};
    let unit = UdpRouteUnit::from_route(mixed_weight_route()).unwrap();
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

impl UdpRouteUnit {
    pub(crate) fn backend_generation(&self) -> &Arc<CompiledL4Backends> {
        &self.backend_finder
    }
}
