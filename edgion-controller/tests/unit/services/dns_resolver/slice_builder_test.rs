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
