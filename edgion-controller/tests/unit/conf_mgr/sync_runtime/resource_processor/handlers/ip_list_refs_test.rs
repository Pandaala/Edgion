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

#[test]
fn actual_global_trust_owner_never_uses_namespaced_grants_or_implicit_scope() {
    use edgion_resources::refs::condition_fields::PreparedConditionReferences;
    use edgion_resources::EdgionGatewayConfig;
    let owner: EdgionGatewayConfig = serde_json::from_value(json!({
        "metadata": {"name": "global"},
        "spec": {"realIp": {"trustedIpRefs": [{"name": "nets", "namespace": "ops"}]}}
    }))
    .unwrap();
    let prepared = PreparedConditionReferences::from_gateway_config(&owner);
    prepared
        .visit_gateway_ip(&owner, |leaf| {
            for (validation, visibility, expected) in [
                (true, "Namespace", IpReadScope::Denied),
                (true, "Cluster", IpReadScope::ClusterOnly),
                (false, "Namespace", IpReadScope::ValidationDisabled),
            ] {
                let outcome = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    validation,
                    |ns, name| {
                        assert_eq!((ns, name), ("ops", "nets"));
                        Some(data(true, visibility, "IpList"))
                    },
                    |_| panic!("cluster RealIp cannot query a namespaced grant"),
                );
                assert_eq!(outcome.read_scope, expected);
                assert_eq!(outcome.denied.is_some(), expected == IpReadScope::Denied);
            }
            for validation in [false, true] {
                let missing = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    validation,
                    |_, _| None,
                    |_| panic!("missing global target cannot query grant"),
                );
                assert_eq!(
                    missing.read_scope,
                    if validation {
                        IpReadScope::ClusterOnly
                    } else {
                        IpReadScope::ValidationDisabled
                    }
                );
                let mut wrong_identity = data(true, "Cluster", "IpList");
                wrong_identity.metadata.name = Some("wrong".into());
                let invalid = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    validation,
                    |_, _| Some(wrong_identity.clone()),
                    |_| panic!("identity failure must precede grants"),
                );
                assert!(invalid.data.is_none());
                let disabled = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    validation,
                    |_, _| Some(data(false, "Cluster", "IpList")),
                    |_| panic!("disabled global trust cannot query grant"),
                );
                assert!(disabled.data.is_none());
            }
        })
        .unwrap();
}

#[test]
fn actual_geo_tokens_use_positive_authority_identity_type_and_permission_precedence() {
    let owner:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"geo","namespace":"default"},"spec":{"requestPlugins":[{"type":"GeoIpLocation","config":{"failOpen":true,"allowRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"nets","namespace":"ops"}]}]}]}}]}})).unwrap();
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    prepared
        .visit_geo_plugins(&owner, |leaf| {
            for (validation, visibility, granted, expected) in [
                (true, "Namespace", false, IpReadScope::Denied),
                (true, "Namespace", true, IpReadScope::GrantAuthorized),
                (true, "Cluster", false, IpReadScope::ClusterOnly),
                (false, "Namespace", false, IpReadScope::ValidationDisabled),
            ] {
                let result = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    validation,
                    |_, _| Some(data(true, visibility, "IpList")),
                    |_| granted,
                );
                assert_eq!(result.read_scope, expected);
            }
            for validation in [true, false] {
                let mut wrong = data(true, "Cluster", "IpList");
                wrong.metadata.namespace = Some("wrong".into());
                let result = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    validation,
                    |_, _| Some(wrong.clone()),
                    |_| panic!("identity check precedes grant"),
                );
                assert!(matches!(result.error, Some(ResolvedRefsError::InvalidReference { .. })));
                for kind in ["IpList", "Selector"] {
                    let result = resolve_ip_reference(
                        leaf.references().unwrap().next().unwrap(),
                        validation,
                        |_, _| Some(data(false, "Cluster", kind)),
                        |_| false,
                    );
                    assert!(result.data.is_none());
                }
            }
        })
        .unwrap();
}

#[test]
fn acl_valid_grant_sibling_keeps_positive_proof_when_malformed_slot_is_omitted() {
    let mut owner: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"owner","namespace":"default"},
    "spec":{"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[
        {"name":"nets","namespace":"ops"},{"name":"bad/name"}
    ]}}]}}))
    .unwrap();
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    let grants = Cell::new(0);
    prepared
        .visit_acl_plugins_mut(&mut owner, |mut leaf| {
            assert!(leaf.is_supported());
            let outcomes: Vec<_> = leaf
                .references(IpListSide::Allow)
                .map(|(token, _)| {
                    resolve_ip_reference(
                        token,
                        true,
                        |namespace, name| {
                            assert_eq!((namespace, name), ("ops", "nets"));
                            Some(data(true, "Namespace", "IpList"))
                        },
                        |query| {
                            grants.set(grants.get() + 1);
                            assert_eq!(query.source().scope().namespace(), Some("default"));
                            assert_eq!(query.target().scope().namespace(), Some("ops"));
                            true
                        },
                    )
                })
                .collect();
            assert!(outcomes[0].data.is_some());
            assert!(matches!(
                outcomes[1].error,
                Some(ResolvedRefsError::InvalidReference { .. })
            ));
            let scopes: Vec<_> = outcomes.iter().map(|outcome| outcome.read_scope).collect();
            assert_eq!(scopes, [IpReadScope::GrantAuthorized, IpReadScope::Denied]);
            leaf.set_read_scopes(IpListSide::Allow, &scopes).unwrap();
        })
        .unwrap();
    assert_eq!(grants.get(), 1);
    let binding = prepared.bind_plugins(&owner).unwrap();
    let proof = binding
        .http_entry(0)
        .unwrap()
        .references()
        .read_scopes(IpListSide::Allow)
        .unwrap()
        .unwrap();
    assert_eq!(proof, [IpReadScope::GrantAuthorized, IpReadScope::Denied]);
    let edgion_resources::EdgionPlugin::IpRestriction(config) = &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
    else {
        panic!()
    };
    let slots = &config.refs.resolved_allow_refs.as_ref().unwrap().slots;
    assert_eq!(slots[0].namespace.as_deref(), Some("ops"));
    assert_eq!(slots[0].name.as_deref(), Some("nets"));
    assert_eq!((slots[1].namespace.as_ref(), slots[1].name.as_ref()), (None, None));
}
