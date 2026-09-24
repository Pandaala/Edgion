use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::udp_route::{UDPBackendRef, UDPRouteRule, UDPRouteSpec};
use kube::api::ObjectMeta;
use std::sync::Arc;

#[tokio::test]
async fn invalid_updates_withdraw_old_route_without_dependency_residue() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor =
        ResourceProcessor::<UDPRoute>::new("UDPRoute", 100, Arc::new(UdpRouteHandler::default()), runtime.clone());
    let rule = |name: &str| UDPRouteRule {
        backend_refs: Some(vec![UDPBackendRef {
            name: name.to_string(),
            namespace: None,
            port: Some(80),
            weight: None,
            group: None,
            kind: None,
            ref_denied: None,
        }]),
    };
    let valid = UDPRoute {
        metadata: ObjectMeta {
            name: Some("r1".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: UDPRouteSpec {
            current_status: None,
            parent_refs: None,
            resolved_status_controller: None,
            rules: Some(vec![rule("svc-a")]),
            resolved_listeners: None,
        },
        status: None,
    };
    let mut invalid = valid.clone();
    invalid.spec.rules.as_mut().unwrap().push(rule("svc-b"));

    processor.on_init_apply(invalid.clone(), None).await;
    assert!(processor.get("default/r1").is_none());
    assert!(runtime.service_ref_mgr.get_refs("default/svc-a").is_empty());
    assert!(runtime.service_ref_mgr.get_refs("default/svc-b").is_empty());

    processor.on_init_apply(valid.clone(), None).await;
    assert!(processor.get("default/r1").is_some());
    processor.on_init_apply(invalid.clone(), None).await;
    processor.on_init_apply(invalid, None).await;
    assert!(processor.get("default/r1").is_none());
    assert!(runtime.service_ref_mgr.get_refs("default/svc-a").is_empty());
    assert!(runtime.service_ref_mgr.get_refs("default/svc-b").is_empty());

    processor.on_init_apply(valid, None).await;
    assert!(processor.get("default/r1").is_some());
}
