use super::*;
use crate::plugins::PluginRuntime;
use crate::routes::http::match_unit::CompiledBackend;
use edgion_resources::HTTPBackendRef;

fn selected_service(name: &str) -> SelectedServiceBackend {
    let compiled = CompiledBackend::compile(
        HTTPBackendRef {
            name: name.to_string(),
            namespace: Some("tenant".to_string()),
            port: Some(8080),
            weight: Some(1),
            group: None,
            kind: Some("Service".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        },
        "route-ns",
        0,
        Arc::new(PluginRuntime::new()),
    );
    let generation = Arc::from(vec![compiled]);
    let logical = LogicalBackendHandle::new(generation, 0).unwrap();
    SelectedServiceBackend::new(logical, BackendTlsPolicyLookup::Absent).unwrap()
}

fn backend_context(service_key: &str) -> crate::BackendContext {
    crate::BackendContext {
        source: None,
        rsc: None,
        service_key: Some(service_key.to_string()),
        service_kind: Some(ServiceBackendKind::Service),
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: Some(0),
        effective_policy: None,
    }
}

#[test]
fn selected_identity_is_the_http_service_resolution_source() {
    let selected = selected_service("service-a");
    let context = backend_context("tenant/service-a");

    let input = selected_http_service_input(&selected, Some(&context)).unwrap();

    assert_eq!(input.port, Some(8080));
    assert_eq!(input.service_key, "tenant/service-a");
    assert_eq!(input.service_type, ServiceBackendKind::Service);
}

#[test]
fn mismatched_context_cannot_redirect_selected_service() {
    let selected = selected_service("service-a");
    let context = backend_context("tenant/service-b");

    assert_eq!(
        selected_http_service_input(&selected, Some(&context)).unwrap_err(),
        EdgionErrorCode::UpstreamNotBackendRefs
    );
}
