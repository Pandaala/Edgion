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
