//! Pure EndpointSlice construction + reconcile decision for the control-plane
//! DNS resolver loop.
//!
//! No I/O lives here — these functions are unit-tested directly. The
//! apiserver-dependent behavior (list Services, apply/delete slices) lives in
//! [`super::service`] and is exercised by the Part C-2 integration suite.

use std::collections::BTreeMap;
use std::net::IpAddr;

use k8s_openapi::api::core::v1::ServicePort;
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::ObjectMeta;

use edgion_resources::constants::annotations::edgion::DNS_RESOLVED_AT as RESOLVED_AT_ANNOTATION;
use edgion_resources::constants::labels::dns::{MANAGED_BY_VALUE, SYNTHETIC as DNS_SYNTHETIC_LABEL};
use edgion_resources::constants::labels::k8s::{ENDPOINTSLICE_MANAGED_BY as MANAGED_BY_LABEL, SERVICE_NAME};

/// Field manager used for the resolver's Server-Side Apply writes (same identity
/// as the `managed-by` label value).
pub const FIELD_MANAGER: &str = MANAGED_BY_VALUE;

/// Deterministic slice name for a Service + address family. A separate name per
/// family is required because an EndpointSlice carries a single `addressType`, so
/// a mixed A/AAAA resolution needs two slices that must not collide.
pub fn slice_name(service_name: &str, is_ipv6: bool) -> String {
    if is_ipv6 {
        format!("{service_name}-edgion-dns-ipv6")
    } else {
        format!("{service_name}-edgion-dns")
    }
}

/// Label selector matching ALL resolver-owned EndpointSlices (any Service, any
/// namespace). The reconcile loop lists these and garbage-collects the ones whose
/// backing Service is no longer a controller-mode ExternalName (or was deleted).
pub fn managed_by_selector() -> String {
    format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}")
}

/// What the resolver loop should do for one Service on a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceAction {
    /// Resolve the ExternalName and write/patch its slice(s).
    Resolve,
    /// Delete the resolver's slice(s) for this Service (switched away / disabled).
    GarbageCollect,
    /// Nothing to do — this Service is never resolver-owned.
    Skip,
}

/// Mirror the gateway's `DnsResolutionMode::parse`: `controller`/absent/unknown
/// all mean controller mode; only `gateway` and `per-request` opt a Service out
/// of the control-plane loop. Keeping this in sync with the gateway parser is
/// what makes the two planes agree on which Services the loop owns.
pub fn is_controller_mode(mode_value: Option<&str>) -> bool {
    !matches!(mode_value.map(str::trim), Some("gateway") | Some("per-request"))
}

/// Decide the action for one Service.
///
/// - Not an ExternalName Service → [`SliceAction::Skip`] (never ours).
/// - Resolver disabled → [`SliceAction::GarbageCollect`] any slice we wrote.
/// - ExternalName + `controller`/absent mode → [`SliceAction::Resolve`].
/// - ExternalName + `gateway`/`per-request` mode → [`SliceAction::GarbageCollect`].
pub fn decide_action(enabled: bool, is_external_name: bool, mode_value: Option<&str>) -> SliceAction {
    if !is_external_name {
        return SliceAction::Skip;
    }
    if !enabled {
        return SliceAction::GarbageCollect;
    }
    if is_controller_mode(mode_value) {
        SliceAction::Resolve
    } else {
        SliceAction::GarbageCollect
    }
}

/// Map the Service's declared ports to EndpointSlice ports. The resolver mirrors
/// the Service port *number* (`ServicePort.port`) onto each `EndpointPort` so the
/// gateway's `ready_endpoints_for_port` — keyed on the route BackendRef port —
/// matches. Names/protocols are carried through for multi-port name resolution.
fn endpoint_ports(service_ports: &[ServicePort]) -> Vec<EndpointPort> {
    service_ports
        .iter()
        .map(|sp| EndpointPort {
            name: sp.name.clone(),
            port: Some(sp.port),
            protocol: sp.protocol.clone(),
            app_protocol: sp.app_protocol.clone(),
        })
        .collect()
}

/// Build the resolver-owned EndpointSlice(s) for a resolved ExternalName Service.
///
/// Returns one slice per address family present in `ips` (an EndpointSlice has a
/// single `addressType`). Every endpoint is `ready=true`; `ports` mirrors the
/// Service's declared ports. Returns empty when there are no resolved IPs or the
/// Service declares no ports — a port-less slice is unroutable (the gateway then
/// bridges via self-resolution), so writing one would be pointless.
pub fn build_endpoint_slices(
    service_name: &str,
    namespace: &str,
    owner: OwnerReference,
    service_ports: &[ServicePort],
    ips: &[IpAddr],
    resolved_at: &str,
) -> Vec<EndpointSlice> {
    let ports = endpoint_ports(service_ports);
    if ports.is_empty() || ips.is_empty() {
        return Vec::new();
    }

    let v4: Vec<IpAddr> = ips.iter().copied().filter(IpAddr::is_ipv4).collect();
    let v6: Vec<IpAddr> = ips.iter().copied().filter(IpAddr::is_ipv6).collect();

    let mut out = Vec::new();
    if !v4.is_empty() {
        out.push(build_one(
            service_name,
            namespace,
            &owner,
            &ports,
            &v4,
            false,
            resolved_at,
        ));
    }
    if !v6.is_empty() {
        out.push(build_one(
            service_name,
            namespace,
            &owner,
            &ports,
            &v6,
            true,
            resolved_at,
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn build_one(
    service_name: &str,
    namespace: &str,
    owner: &OwnerReference,
    ports: &[EndpointPort],
    ips: &[IpAddr],
    is_ipv6: bool,
    resolved_at: &str,
) -> EndpointSlice {
    let labels = BTreeMap::from([
        (SERVICE_NAME.to_string(), service_name.to_string()),
        (DNS_SYNTHETIC_LABEL.to_string(), "true".to_string()),
        (MANAGED_BY_LABEL.to_string(), MANAGED_BY_VALUE.to_string()),
    ]);
    let annotations = BTreeMap::from([(RESOLVED_AT_ANNOTATION.to_string(), resolved_at.to_string())]);

    let endpoints = ips
        .iter()
        .map(|ip| Endpoint {
            addresses: vec![ip.to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(true),
                serving: Some(true),
                terminating: Some(false),
            }),
            ..Default::default()
        })
        .collect();

    EndpointSlice {
        metadata: ObjectMeta {
            name: Some(slice_name(service_name, is_ipv6)),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            annotations: Some(annotations),
            owner_references: Some(vec![owner.clone()]),
            ..Default::default()
        },
        address_type: if is_ipv6 { "IPv6" } else { "IPv4" }.to_string(),
        endpoints,
        ports: Some(ports.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> OwnerReference {
        OwnerReference {
            api_version: "v1".to_string(),
            kind: "Service".to_string(),
            name: "ext".to_string(),
            uid: "uid-123".to_string(),
            controller: Some(false),
            block_owner_deletion: Some(false),
        }
    }

    fn svc_port(name: Option<&str>, port: i32) -> ServicePort {
        ServicePort {
            name: name.map(String::from),
            port,
            ..Default::default()
        }
    }

    // ── is_controller_mode / decide_action ────────────────────────────────────

    #[test]
    fn controller_mode_covers_absent_and_unknown() {
        assert!(is_controller_mode(None));
        assert!(is_controller_mode(Some("controller")));
        assert!(is_controller_mode(Some("  controller ")));
        assert!(is_controller_mode(Some("bogus")));
        assert!(!is_controller_mode(Some("gateway")));
        assert!(!is_controller_mode(Some("per-request")));
    }

    #[test]
    fn decide_action_matrix() {
        // Non-ExternalName is never ours, even when disabled.
        assert_eq!(decide_action(true, false, None), SliceAction::Skip);
        assert_eq!(decide_action(false, false, Some("controller")), SliceAction::Skip);
        // ExternalName + controller mode + enabled → resolve.
        assert_eq!(decide_action(true, true, None), SliceAction::Resolve);
        assert_eq!(decide_action(true, true, Some("controller")), SliceAction::Resolve);
        // ExternalName but opted into a gateway path → GC our slice.
        assert_eq!(decide_action(true, true, Some("gateway")), SliceAction::GarbageCollect);
        assert_eq!(
            decide_action(true, true, Some("per-request")),
            SliceAction::GarbageCollect
        );
        // Disabled resolver → GC our ExternalName slices.
        assert_eq!(
            decide_action(false, true, Some("controller")),
            SliceAction::GarbageCollect
        );
    }

    // ── slice_name / gc_label_selector ────────────────────────────────────────

    #[test]
    fn slice_names_are_family_scoped() {
        assert_eq!(slice_name("ext", false), "ext-edgion-dns");
        assert_eq!(slice_name("ext", true), "ext-edgion-dns-ipv6");
    }

    #[test]
    fn managed_by_selector_targets_all_resolver_slices() {
        assert_eq!(
            managed_by_selector(),
            "endpointslice.kubernetes.io/managed-by=edgion-dns-resolver"
        );
    }

    // ── build_endpoint_slices ─────────────────────────────────────────────────

    #[test]
    fn no_ports_yields_no_slice() {
        let ips = vec![IpAddr::from([10, 0, 0, 1])];
        assert!(build_endpoint_slices("ext", "app", owner(), &[], &ips, "now").is_empty());
    }

    #[test]
    fn no_ips_yields_no_slice() {
        assert!(build_endpoint_slices("ext", "app", owner(), &[svc_port(None, 80)], &[], "now").is_empty());
    }

    #[test]
    fn ipv4_slice_has_required_labels_owner_port_and_ready_endpoints() {
        let ips = vec![IpAddr::from([10, 0, 0, 1]), IpAddr::from([10, 0, 0, 2])];
        let slices = build_endpoint_slices(
            "ext",
            "app",
            owner(),
            &[svc_port(Some("http"), 80)],
            &ips,
            "2026-07-07T00:00:00Z",
        );
        assert_eq!(slices.len(), 1, "single family → single slice");
        let s = &slices[0];

        assert_eq!(s.metadata.name.as_deref(), Some("ext-edgion-dns"));
        assert_eq!(s.metadata.namespace.as_deref(), Some("app"));
        assert_eq!(s.address_type, "IPv4");

        let labels = s.metadata.labels.as_ref().unwrap();
        assert_eq!(labels.get(SERVICE_NAME).map(String::as_str), Some("ext"));
        assert_eq!(labels.get(DNS_SYNTHETIC_LABEL).map(String::as_str), Some("true"));
        assert_eq!(labels.get(MANAGED_BY_LABEL).map(String::as_str), Some(MANAGED_BY_VALUE));

        let anns = s.metadata.annotations.as_ref().unwrap();
        assert_eq!(
            anns.get(RESOLVED_AT_ANNOTATION).map(String::as_str),
            Some("2026-07-07T00:00:00Z")
        );

        let refs = s.metadata.owner_references.as_ref().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].uid, "uid-123");
        assert_eq!(refs[0].kind, "Service");

        // Port mirrors the Service port number (what the gateway keys on).
        let ports = s.ports.as_ref().unwrap();
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, Some(80));
        assert_eq!(ports[0].name.as_deref(), Some("http"));

        // One ready endpoint per resolved IP.
        assert_eq!(s.endpoints.len(), 2);
        for ep in &s.endpoints {
            assert_eq!(ep.conditions.as_ref().and_then(|c| c.ready), Some(true));
        }
        let addrs: Vec<&str> = s
            .endpoints
            .iter()
            .flat_map(|e| e.addresses.iter().map(String::as_str))
            .collect();
        assert!(addrs.contains(&"10.0.0.1"));
        assert!(addrs.contains(&"10.0.0.2"));
    }

    #[test]
    fn mixed_families_split_into_two_addresstype_scoped_slices() {
        let ips = vec![IpAddr::from([10, 0, 0, 1]), "2001:db8::1".parse().unwrap()];
        let slices = build_endpoint_slices("ext", "app", owner(), &[svc_port(None, 443)], &ips, "now");
        assert_eq!(slices.len(), 2);
        let v4 = slices.iter().find(|s| s.address_type == "IPv4").unwrap();
        let v6 = slices.iter().find(|s| s.address_type == "IPv6").unwrap();
        assert_eq!(v4.metadata.name.as_deref(), Some("ext-edgion-dns"));
        assert_eq!(v6.metadata.name.as_deref(), Some("ext-edgion-dns-ipv6"));
        assert_eq!(v4.endpoints.len(), 1);
        assert_eq!(v6.endpoints.len(), 1);
        assert_eq!(v6.endpoints[0].addresses, vec!["2001:db8::1".to_string()]);
    }

    #[test]
    fn multi_port_service_mirrors_all_ports() {
        let ips = vec![IpAddr::from([10, 0, 0, 1])];
        let ports = [svc_port(Some("http"), 80), svc_port(Some("https"), 443)];
        let slices = build_endpoint_slices("ext", "app", owner(), &ports, &ips, "now");
        let s = &slices[0];
        let eps = s.ports.as_ref().unwrap();
        let nums: Vec<i32> = eps.iter().filter_map(|p| p.port).collect();
        assert_eq!(nums, vec![80, 443]);
    }
}
