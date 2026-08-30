use crate::backends::discovery::{get_global_service_store, get_roundrobin_store};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;

/// Resolve the correct targetPort for a backend address selected from the load balancer.
///
/// The LB stores backends with the targetPort from the first port entry in EndpointSlice.
/// For multi-port Services, `br_port` (backendRef.port = Service port number) may refer to a
/// different port entry. This function looks up the correct targetPort via:
///   br_port (Service port) -> Service.spec.ports[].name -> EndpointSlice port name -> targetPort
///
/// For single-port Services where port != targetPort (e.g. port:443 -> targetPort:8443),
/// the addr may still carry the Service port if the LB was built before EndpointSlice sync.
/// This function corrects it by looking up the targetPort directly from the Service spec.
pub fn resolve_target_port_key(addr: &mut SocketAddr, br_port: Option<i32>, service_key: &str) {
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
    let port_name = svc_port_entry
        .name
        .as_deref()
        .expect("name is Some and non-empty: single-port and empty-name branches returned early");

    // A named port must be resolved against the slice the SELECTED endpoint
    // actually came from: sibling slices of one Service may map the same port
    // name to different numbers (a named `targetPort` resolves per-pod, so a
    // rollout can leave heterogeneous slices side by side). Locate the
    // originating slice by the selected endpoint's IP, then resolve the name
    // within it. Only when the origin is unknown (endpoint not found under
    // partial sync) fall back to a cross-slice match, using `min` so
    // non-deterministic HashSet iteration order cannot flip the result across
    // rebuilds.
    let selected_ip = addr.as_inet().map(|a| a.ip());
    let resolve_in_slice = |s: &EndpointSlice| -> Option<u16> {
        s.ports.as_ref()?.iter().find_map(|p| {
            if p.name.as_deref() == Some(port_name) {
                p.port.map(|v| v as u16)
            } else {
                None
            }
        })
    };
    let resolved = get_roundrobin_store()
        .get_slices_for_service(service_key)
        .and_then(|slices| {
            let from_origin = selected_ip.and_then(|ip| {
                slices
                    .iter()
                    .find(|s| slice_contains_ip(s, ip))
                    .and_then(&resolve_in_slice)
            });
            from_origin.or_else(|| slices.iter().filter_map(&resolve_in_slice).min())
        });

    if let Some(target_port) = resolved {
        let current_port = addr.as_inet().map(|a| a.port()).unwrap_or(0);
        if current_port != target_port {
            addr.set_port(target_port);
        }
    }
}

/// True when the EndpointSlice lists `ip` among any endpoint's addresses.
///
/// Addresses are parsed before comparison so equality is numeric rather than
/// string-shaped (canonical vs. non-canonical IPv6 forms compare equal).
fn slice_contains_ip(slice: &EndpointSlice, ip: std::net::IpAddr) -> bool {
    slice.endpoints.iter().any(|ep| {
        ep.addresses.iter().any(|a| {
            a.parse::<std::net::IpAddr>()
                .map(|parsed| parsed == ip)
                .unwrap_or(false)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use std::collections::{BTreeMap, HashMap};

    fn named_slice(name: &str, service: &str, addr: &str, port_name: &str, port: i32) -> EndpointSlice {
        let mut labels = BTreeMap::new();
        labels.insert(SERVICE_NAME.to_string(), service.to_string());
        EndpointSlice {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: vec![Endpoint {
                addresses: vec![addr.to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ports: Some(vec![EndpointPort {
                name: Some(port_name.to_string()),
                port: Some(port),
                ..Default::default()
            }]),
        }
    }

    // Two slices of one Service map the same port name to different numbers
    // (heterogeneous named ports mid-rollout). The rewrite must keep each
    // selected endpoint on its own slice's port, not collapse to the first
    // slice that declares the name.
    #[test]
    fn named_port_resolves_against_originating_slice() {
        let service_key = "default/svc-port-resolver-hetero";
        let service_short = "svc-port-resolver-hetero";

        // Multi-port Service so the named branch is taken; the "http" entry has
        // a non-empty name and there are >= 2 ports.
        let service = Service {
            metadata: ObjectMeta {
                name: Some(service_short.to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                ports: Some(vec![
                    ServicePort {
                        name: Some("http".to_string()),
                        port: 80,
                        target_port: Some(IntOrString::String("http".to_string())),
                        ..Default::default()
                    },
                    ServicePort {
                        name: Some("grpc".to_string()),
                        port: 90,
                        target_port: Some(IntOrString::String("grpc".to_string())),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut svc_add = HashMap::new();
        svc_add.insert(service_key.to_string(), service);
        get_global_service_store().update(svc_add, &Default::default());

        // slice A: http -> 8080 for 10.9.9.1 ; slice B: http -> 9090 for 10.9.9.2
        let mut slices = HashMap::new();
        slices.insert(
            "default/svc-port-resolver-hetero-a".to_string(),
            named_slice("svc-port-resolver-hetero-a", service_short, "10.9.9.1", "http", 8080),
        );
        slices.insert(
            "default/svc-port-resolver-hetero-b".to_string(),
            named_slice("svc-port-resolver-hetero-b", service_short, "10.9.9.2", "http", 9090),
        );
        get_roundrobin_store().update_data_only(slices, HashMap::new(), &Default::default());

        // Endpoint from slice B keeps 9090 (its own slice), not slice A's 8080.
        let mut addr_b = "10.9.9.2:9090".parse::<SocketAddr>().unwrap();
        resolve_target_port_key(&mut addr_b, Some(80), service_key);
        assert_eq!(addr_b.as_inet().unwrap().port(), 9090);

        // Endpoint from slice A keeps 8080.
        let mut addr_a = "10.9.9.1:8080".parse::<SocketAddr>().unwrap();
        resolve_target_port_key(&mut addr_a, Some(80), service_key);
        assert_eq!(addr_a.as_inet().unwrap().port(), 8080);

        // Unknown originating endpoint (IP not in any slice) falls back to the
        // deterministic cross-slice minimum (8080 < 9090), never HashSet order.
        let mut addr_unknown = "10.9.9.250:1".parse::<SocketAddr>().unwrap();
        resolve_target_port_key(&mut addr_unknown, Some(80), service_key);
        assert_eq!(addr_unknown.as_inet().unwrap().port(), 8080);
    }
}

/// Resolve a Service port number to its port name.
pub(crate) fn resolve_port_name(service_name: &str, namespace: Option<&str>, port: i32) -> Option<String> {
    let key = format!("{}/{}", namespace.unwrap_or("default"), service_name);
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
