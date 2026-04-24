use crate::core::gateway::backends::discovery::{
    get_endpoint_roundrobin_store, get_global_service_store, get_roundrobin_store,
};
use crate::core::gateway::backends::endpoint_mode::{get_global_endpoint_mode, EndpointMode};
use pingora_core::protocols::l4::socket::SocketAddr;

/// Which endpoint resource to query when resolving targetPort by port name.
#[derive(Debug, Clone, Copy)]
pub enum PortLookupSource {
    /// Determine from global endpoint mode (for `EdgionService::Service`).
    Auto,
    /// Force EndpointSlice (for `EdgionService::ServiceEndpointSlice`).
    EndpointSlice,
    /// Force Endpoint (for `EdgionService::ServiceEndpoint`).
    Endpoint,
}

/// Resolve the correct targetPort for a backend address selected from the load balancer.
///
/// The LB stores backends with the targetPort from the first port entry in EndpointSlice/Endpoint.
/// For multi-port Services, `br_port` (backendRef.port = Service port number) may refer to a
/// different port entry. This function looks up the correct targetPort via:
///   br_port (Service port) -> Service.spec.ports[].name -> EndpointSlice/Endpoint port name -> targetPort
///
/// For single-port Services where port != targetPort (e.g. port:443 -> targetPort:8443),
/// the addr may still carry the Service port if the LB was built before EndpointSlice sync.
/// This function corrects it by looking up the targetPort directly from the Service spec.
pub fn resolve_target_port(
    addr: &mut SocketAddr,
    br_port: Option<i32>,
    service_key: &str,
    source: PortLookupSource,
) {
    let Some(br_port_val) = br_port else {
        return;
    };

    let svc_store = get_global_service_store();
    let Some(service) = svc_store.get(service_key) else {
        return;
    };

    let Some(svc_ports) = service.spec.as_ref().and_then(|s| s.ports.as_ref()) else {
        return;
    };

    let Some(svc_port_entry) = svc_ports.iter().find(|p| p.port == br_port_val) else {
        return;
    };

    if svc_ports.len() <= 1 || svc_port_entry.name.as_deref().map_or(true, |n| n.is_empty()) {
        if let Some(tp) = &svc_port_entry.target_port {
            let target = match tp {
                k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(p) => Some(*p as u16),
                k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(s) => s.parse::<u16>().ok(),
            };
            if let Some(target_port) = target {
                let current_port = addr.as_inet().map(|a| a.port()).unwrap_or(0);
                if current_port != target_port {
                    addr.set_port(target_port);
                }
            }
        }
        return;
    }

    // SAFETY: the early-return at the single-port branch ensures `name` is Some and non-empty here.
    let port_name = svc_port_entry.name.as_deref().unwrap();

    let use_endpoint_slice = match source {
        PortLookupSource::EndpointSlice => true,
        PortLookupSource::Endpoint => false,
        PortLookupSource::Auto => !matches!(get_global_endpoint_mode(), EndpointMode::Endpoint),
    };

    let resolved = if use_endpoint_slice {
        get_roundrobin_store()
            .get_slices_for_service(service_key)
            .and_then(|slices| {
                slices.iter().find_map(|s| {
                    s.ports.as_ref()?.iter().find_map(|p| {
                        if p.name.as_deref() == Some(port_name) {
                            p.port.map(|v| v as u16)
                        } else {
                            None
                        }
                    })
                })
            })
    } else {
        get_endpoint_roundrobin_store()
            .get_endpoint_for_service(service_key)
            .and_then(|ep| {
                ep.subsets.as_ref()?.iter().find_map(|subset| {
                    subset.ports.as_ref()?.iter().find_map(|p| {
                        if p.name.as_deref() == Some(port_name) {
                            Some(p.port as u16)
                        } else {
                            None
                        }
                    })
                })
            })
    };

    if let Some(target_port) = resolved {
        let current_port = addr.as_inet().map(|a| a.port()).unwrap_or(0);
        if current_port != target_port {
            addr.set_port(target_port);
        }
    }
}

/// Resolve a Service port number to its port name.
pub(crate) fn resolve_port_name(service_name: &str, namespace: Option<&str>, port: i32) -> Option<String> {
    let ns = namespace.unwrap_or("");
    let key = format!("{}/{}", ns, service_name);
    let svc_store = get_global_service_store();
    svc_store.with_service(&key, |svc| {
        svc.spec
            .as_ref()?
            .ports
            .as_ref()?
            .iter()
            .find_map(|sp| if sp.port == port { sp.name.clone() } else { None })
    })?
}
