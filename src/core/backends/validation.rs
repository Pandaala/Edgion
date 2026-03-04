use std::net::IpAddr;

use crate::core::backends::{get_endpoint_roundrobin_store, get_global_endpoint_mode, get_roundrobin_store};
use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::types::HTTPBackendRef;

/// Validate that a given IP is a valid endpoint for one of the route's backend_refs.
///
/// Returns:
/// - `Ok((backend_ref_index, port))` if the IP is found in any backend_ref's endpoints
/// - `Err(reason)` if the IP is not found
///
/// Security: This prevents SSRF by ensuring the caller can only route to
/// endpoints that the route already has access to via its backend_refs.
pub fn validate_endpoint_in_route(
    target_ip: &IpAddr,
    target_port: Option<u16>,
    route_backend_refs: &[HTTPBackendRef],
    route_namespace: &str,
) -> Result<(usize, u16), String> {
    for (idx, br) in route_backend_refs.iter().enumerate() {
        let br_namespace = br.namespace.as_deref().unwrap_or(route_namespace);
        let service_key = format!("{}/{}", br_namespace, br.name);
        let br_port = br.port.map(|p| p as u16);

        // Determine effective port
        let Some(port) = target_port.or(br_port) else {
            continue; // No port available for this backend_ref
        };

        // Check Endpoints or EndpointSlice based on global mode
        if is_ip_in_service_endpoints(target_ip, port, &service_key) {
            return Ok((idx, port));
        }
    }

    Err(format!("IP {} not found in any backend_ref endpoints", target_ip))
}

/// Check if a given IP:port exists in a Service's endpoints
fn is_ip_in_service_endpoints(target_ip: &IpAddr, port: u16, service_key: &str) -> bool {
    let mode = get_global_endpoint_mode();

    match mode {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => {
            let store = get_roundrobin_store(); // EndpointSlice store
            if let Some(slices) = store.get_slices_for_service(service_key) {
                for slice in &slices {
                    if check_endpoint_slice_with_port(slice, target_ip, port) {
                        return true;
                    }
                }
            }
            if matches!(mode, EndpointMode::Both) {
                // Fallback to legacy Endpoints
                return check_legacy_endpoints(target_ip, port, service_key);
            }
            false
        }
        EndpointMode::Endpoint => check_legacy_endpoints(target_ip, port, service_key),
    }
}

fn check_endpoint_slice_with_port(
    slice: &k8s_openapi::api::discovery::v1::EndpointSlice,
    target_ip: &IpAddr,
    port: u16,
) -> bool {
    // Check if port exists in slice ports
    let has_port = slice
        .ports
        .as_ref()
        .is_some_and(|ports| ports.iter().any(|p| p.port == Some(port as i32)));
    if !has_port {
        return false;
    }

    let target_ip_str = target_ip.to_string();

    // Check IP
    for ep in &slice.endpoints {
        for addr in &ep.addresses {
            if addr == &target_ip_str {
                return true;
            }
        }
    }
    false
}

fn check_legacy_endpoints(target_ip: &IpAddr, port: u16, service_key: &str) -> bool {
    let store = get_endpoint_roundrobin_store();
    if let Some(ep) = store.get_endpoint_for_service(service_key) {
        let target_ip_str = target_ip.to_string();

        if let Some(subsets) = &ep.subsets {
            for subset in subsets {
                // Check port
                let has_port = subset
                    .ports
                    .as_ref()
                    .is_some_and(|ports| ports.iter().any(|p| p.port == port as i32));
                if !has_port {
                    continue;
                }

                // Check IP
                if let Some(addresses) = &subset.addresses {
                    if addresses.iter().any(|addr| addr.ip == target_ip_str) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    #[test]
    fn test_check_endpoint_slice_with_port() {
        let slice = EndpointSlice {
            metadata: ObjectMeta::default(),
            address_type: "IPv4".to_string(),
            endpoints: vec![Endpoint {
                addresses: vec!["10.0.0.1".to_string()],
                ..Default::default()
            }],
            ports: Some(vec![k8s_openapi::api::discovery::v1::EndpointPort {
                port: Some(8080),
                ..Default::default()
            }]),
        };

        let ip_valid: IpAddr = "10.0.0.1".parse().unwrap();
        let ip_invalid: IpAddr = "10.0.0.2".parse().unwrap();

        assert!(check_endpoint_slice_with_port(&slice, &ip_valid, 8080));
        assert!(!check_endpoint_slice_with_port(&slice, &ip_valid, 9090)); // Wrong port
        assert!(!check_endpoint_slice_with_port(&slice, &ip_invalid, 8080)); // Wrong IP
    }
}
