use super::*;
use std::cell::Cell;

fn ip_owner(reference: serde_json::Value) -> EdgionPlugins {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"owner"},"spec":{"requestPlugins":[{
        "type":"RequestRestriction","config":{"mode":"Deny","conditions":{"allOf":[{"type":"remoteIpMatch","ipRefs":[reference]}]}}
    }]}})).unwrap()
}
fn data(enable: bool, visibility: &str, kind: &str) -> EdgionConfigData {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"nets","namespace":"ops"},"spec":{
        "enable":enable,"visibility":visibility,"data":{"type":kind,"config":{"items":[]}}
    }}))
    .unwrap()
}

#[test]
fn invalid_ip_identity_crosses_neither_read_boundary() {
    for wire in [
        serde_json::json!({"name":""}),
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"nets","namespace":""}),
        serde_json::json!({"name":"nets","namespace":"bad/ns"}),
    ] {
        let owner = ip_owner(wire);
        let prepared = PreparedConditionReferences::from_plugins(&owner);
        let reads = Cell::new(0);
        let grants = Cell::new(0);
        prepared
            .visit_ip(&owner, |leaf| {
                let result = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    false,
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
            })
            .unwrap();
        assert_eq!((reads.get(), grants.get()), (0, 0));
    }
}

#[test]
fn ip_permission_precedence_and_namespace_defaults_remain_field_specific() {
    for (namespace, enabled, visibility, flag, grant_allowed, denied) in [
        (None, false, "Namespace", true, false, false),
        (Some("ops"), false, "Namespace", true, false, true),
        (Some("ops"), true, "Namespace", true, true, false),
        (Some("ops"), true, "Namespace", false, false, false),
        (Some("ops"), true, "Cluster", true, false, false),
    ] {
        let owner = ip_owner(serde_json::json!({"name":"nets","namespace":namespace}));
        let prepared = PreparedConditionReferences::from_plugins(&owner);
        let grants = Cell::new(0);
        prepared
            .visit_ip(&owner, |leaf| {
                let result = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    flag,
                    |ns, name| {
                        assert_eq!(ns, namespace.unwrap_or("default"));
                        assert_eq!(name, "nets");
                        Some(data(enabled, visibility, "IpList"))
                    },
                    |query| {
                        grants.set(grants.get() + 1);
                        assert_eq!(query.source().scope().namespace(), Some("default"));
                        assert_eq!(query.source().group_kind().kind(), "EdgionPlugins");
                        grant_allowed
                    },
                );
                assert_eq!(result.denied.is_some(), denied);
                if denied {
                    assert!(matches!(result.error, Some(ResolvedRefsError::RefNotPermitted { .. })));
                } else if !enabled {
                    assert!(matches!(
                        result.error,
                        Some(ResolvedRefsError::EdgionConfigDataNotFound { .. })
                    ));
                } else {
                    assert!(result.error.is_none());
                }
            })
            .unwrap();
        assert_eq!(
            grants.get(),
            usize::from(namespace.is_some() && flag && visibility == "Namespace")
        );
    }
    let owner = ip_owner(serde_json::json!({"name":"nets","namespace":"ops"}));
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    for target in [None, Some(data(true, "Namespace", "Selector"))] {
        prepared
            .visit_ip(&owner, |leaf| {
                let result = resolve_ip_reference(
                    leaf.references().unwrap().next().unwrap(),
                    true,
                    |_, _| target.clone(),
                    |_| panic!("unavailable target must not query Grant"),
                );
                assert!(matches!(
                    result.error,
                    Some(ResolvedRefsError::EdgionConfigDataNotFound { .. })
                ));
                assert!(result.denied.is_none());
            })
            .unwrap();
    }
}
