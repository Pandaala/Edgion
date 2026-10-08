use super::*;
use serde_json::json;
use std::cell::Cell;
fn owner(reference: serde_json::Value) -> EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[reference]}}]}})).unwrap()
}
fn data(enable: bool, visibility: &str, kind: &str) -> EdgionConfigData {
    serde_json::from_value(json!({"metadata":{"name":"nets","namespace":"ops"},"spec":{
        "enable":enable,"visibility":visibility,"data":{"type":kind,"config":{"items":[]}}
    }}))
    .unwrap()
}
#[test]
fn malformed_targets_and_secret_role_cross_zero_read_or_grant_boundaries() {
    let reads = Cell::new(0);
    let grants = Cell::new(0);
    fn check(token: ReferenceToken<'_>, reads: &Cell<usize>, grants: &Cell<usize>) {
        let result = resolve_ip_reference(
            token,
            true,
            |_, _| {
                reads.set(reads.get() + 1);
                None
            },
            |_| {
                grants.set(grants.get() + 1);
                true
            },
        );
        assert!(matches!(result.error, Some(ResolvedRefsError::InvalidReference { .. })));
        assert!(result.denied.is_none());
    }
    for target in [
        json!({"name":""}),
        json!({"name":"bad/name"}),
        json!({"name":"nets","namespace":""}),
        json!({"name":"nets","namespace":"bad/ns"}),
    ] {
        let owner = owner(target);
        let prepared = PreparedIpListReferences::from_plugins(&owner);
        let binding = prepared.bind_plugins(&owner).unwrap();
        let refs = binding.http_entry(0).unwrap().references();
        check(refs.allow().next().unwrap().0, &reads, &grants);
        assert!(http_pairs(&owner).is_empty());
    }
    let owner: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"requestPlugins":[{"type":"Csrf","config":{"secretRef":{"name":"nets"}}}]}})).unwrap();
    let prepared = edgion_resources::refs::plugin_fields::PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let refs = entry.references().unwrap();
    check(refs.csrf_secret().unwrap(), &reads, &grants);
    assert_eq!((reads.get(), grants.get()), (0, 0));
}
#[test]
fn shared_permission_precedence_preserves_unavailable_denied_and_disabled() {
    for (kind, enabled, visibility, flag, allowed, denied, grant_count) in [
        ("Selector", false, "Namespace", true, false, false, 0),
        ("IpList", false, "Namespace", true, false, true, 1),
        ("IpList", false, "Namespace", true, true, false, 1),
        ("IpList", true, "Cluster", true, false, false, 0),
        ("IpList", true, "Namespace", false, false, false, 0),
    ] {
        let owner = owner(json!({"name":"nets","namespace":"ops"}));
        let prepared = PreparedIpListReferences::from_plugins(&owner);
        let binding = prepared.bind_plugins(&owner).unwrap();
        let refs = binding.http_entry(0).unwrap().references();
        let grants = Cell::new(0);
        let result = resolve_ip_reference(
            refs.allow().next().unwrap().0,
            flag,
            |ns, name| {
                assert_eq!((ns, name), ("ops", "nets"));
                Some(data(enabled, visibility, kind))
            },
            |query| {
                grants.set(grants.get() + 1);
                assert_eq!(query.source().scope().namespace(), Some("default"));
                allowed
            },
        );
        assert_eq!(result.denied.is_some(), denied);
        assert_eq!(grants.get(), grant_count);
        assert_eq!(result.data.is_some(), kind == "IpList" && enabled && !denied);
    }
}
