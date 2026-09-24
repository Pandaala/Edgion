use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::Gateway;
use std::sync::Arc;

#[tokio::test]
async fn invalid_hostnames_clear_attachment_status_and_counts_then_recover() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    raw.upsert(
        "apps/gateway".into(),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "metadata": {"namespace": "apps", "name": "gateway"},
                "spec": {"gatewayClassName": "test", "listeners": [{
                    "name": "tls", "port": 8443, "protocol": "TLS",
                    "tls": {"mode": "Passthrough"}
                }]}
            }))
            .unwrap(),
        ),
    );
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    let handler = TlsRouteHandler::default();
    let mut route: TLSRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "route"},
        "spec": {"parentRefs": [{"name": "gateway", "sectionName": "tls"}]}
    }))
    .unwrap();
    let valid = Some(vec!["a.example.com".to_string()]);
    for hostnames in [
        valid.clone(),
        Some(Vec::new()),
        valid.clone(),
        None,
        valid.clone(),
        Some(vec!["*".to_string()]),
        Some(vec!["a.example.com".to_string(); 16]),
        Some(vec!["a.example.com".to_string(); 17]),
        Some(vec!["a.example.com".to_string(), "*".to_string()]),
        Some(vec!["127.0.0.1".to_string()]),
        Some(vec!["*.127.0.0.1".to_string()]),
        valid,
    ] {
        route.spec.hostnames = hostnames;
        let errors = handler.validate_route(&route);
        let accepted = errors.is_empty() && route.spec.validate_hostnames().is_ok();
        let ProcessResult::Continue(mut parsed) = handler.parse_route(route, &ctx).await else {
            panic!("route parsing must retain the object for status reporting");
        };
        handler.update_route_status(&mut parsed, &ctx, &errors);
        let parents = &parsed.status.as_ref().unwrap().parents;
        assert_eq!(parents.len(), 1);
        let condition = parents[0].conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(condition.status, if accepted { "True" } else { "False" });
        assert_eq!(condition.reason, if accepted { "Accepted" } else { "UnsupportedValue" });
        assert_eq!(parsed.spec.resolved_listeners.is_some(), accepted);
        assert_eq!(
            runtime.attached_route_tracker.count_for_listener("apps/gateway", "tls"),
            i32::from(accepted)
        );
        route = parsed;
    }
}
