use super::*;

#[test]
fn groups_are_exact_and_scope_is_explicit() {
    let core = ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced("team"), "key").unwrap();
    let literal_core = ObjectIdentity::checked("core", "Secret", ResourceScope::Namespaced("team"), "key").unwrap();
    assert_ne!(core, literal_core);
    assert_eq!(core.group_kind().group(), "");
    assert_eq!(core.group_kind().kind(), "Secret");
    assert_eq!(core.scope().namespace(), Some("team"));
    assert_eq!(core.name(), "key");
    let cluster =
        ObjectIdentity::checked("edgion.io", "EdgionGatewayConfig", ResourceScope::Cluster, "global").unwrap();
    assert_eq!(cluster.scope().namespace(), None);
    assert_ne!(cluster.scope(), ResourceScope::Namespaced("default"));
}

#[test]
fn invalid_identity_is_rejected_without_normalizing_input() {
    for group in ["Core", " core", "core ", "bad/group"] {
        assert_eq!(GroupKind::checked(group, "Secret"), Err(ReferenceError::InvalidGroup));
    }
    for kind in ["", " Secret", "Secret ", "1Secret", "Secret/Object", "Secret-"] {
        assert_eq!(GroupKind::checked("", kind), Err(ReferenceError::InvalidKind));
    }
    for namespace in ["", "Team", "team/ns"] {
        assert_eq!(
            ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced(namespace), "key"),
            Err(ReferenceError::InvalidNamespace)
        );
    }
    for name in ["", "*", "Key", "team/key", " key"] {
        assert_eq!(
            ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced("team"), name),
            Err(ReferenceError::InvalidName)
        );
    }
}

#[test]
fn kubernetes_object_names_bound_total_length_without_a_component_cap() {
    for kind in ["Secret", "ConfigMap", "EdgionPlugins", "HTTPRoute"] {
        let group = match kind {
            "EdgionPlugins" => "edgion.io",
            "HTTPRoute" => "gateway.networking.k8s.io",
            _ => "",
        };
        for length in [63, 64, 253, 254] {
            let name = "a".repeat(length);
            let identity = ObjectIdentity::checked(group, kind, ResourceScope::Namespaced("apps"), &name);
            if length <= 253 {
                assert_eq!(identity.unwrap().name(), name);
            } else {
                assert_eq!(identity, Err(ReferenceError::InvalidName));
            }
        }
    }
    for name in [format!("{}.b-2", "a".repeat(64)), format!("{}.b", "a".repeat(251))] {
        assert!(ObjectIdentity::checked("", "Secret", ResourceScope::Cluster, &name).is_ok());
    }
    for name in [
        "", ".a", "a.", "a..b", "-a", "a-", "a.-b", "a-.b", "A", "a_b", "a/b", "a b",
    ] {
        assert_eq!(
            ObjectIdentity::checked("", "Secret", ResourceScope::Cluster, name),
            Err(ReferenceError::InvalidName)
        );
    }
    let group = "a".repeat(64);
    assert_eq!(GroupKind::checked(&group, "Secret").unwrap().group(), group);
    assert_ne!(GroupKind::checked("core", "Secret"), GroupKind::checked("", "Secret"));
}

#[test]
fn resource_subdomains_keep_namespace_labels_and_native_object_names_distinct() {
    let namespace = "a".repeat(63);
    assert!(ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced(&namespace), "key").is_ok());
    for namespace in ["a".repeat(64), "apps.team".into()] {
        for native in [false, true] {
            let identity = if native {
                ObjectIdentity::native_object_reference("", "Secret", ResourceScope::Namespaced(&namespace), "key")
            } else {
                ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced(&namespace), "key")
            };
            assert_eq!(identity, Err(ReferenceError::InvalidNamespace));
        }
    }
    for name in ["Capital_Name".to_string(), "é".repeat(253)] {
        assert!(ObjectIdentity::native_object_reference("", "Secret", ResourceScope::Cluster, &name).is_ok());
        assert_eq!(
            ObjectIdentity::checked("", "Secret", ResourceScope::Cluster, &name),
            Err(ReferenceError::InvalidName)
        );
    }
    assert!(ObjectIdentity::native_object_reference("", "Secret", ResourceScope::Cluster, &"é".repeat(254)).is_err());
}
