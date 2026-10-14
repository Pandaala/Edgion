use super::ResourceKind;

#[test]
fn edgion_backend_exclusively_owns_wire_value_23() {
    assert_eq!(ResourceKind::EdgionBackend as i32, 23);
    assert_eq!(ResourceKind::try_from(23).ok(), Some(ResourceKind::EdgionBackend));
    assert_eq!(ResourceKind::EdgionBackend.as_str(), "EdgionBackend");
    assert_eq!(
        ResourceKind::from_kind_name("EdgionBackend"),
        Some(ResourceKind::EdgionBackend)
    );
    assert_eq!(ResourceKind::from_kind_name("UnknownResource"), None);
}

#[test]
fn kind_name_mapping_preserves_case_and_aliases() {
    for (name, expected) in [
        ("hTtPrOuTe", ResourceKind::HTTPRoute),
        ("EdgionGwConfig", ResourceKind::EdgionGatewayConfig),
        ("ReferenceGrants", ResourceKind::ReferenceGrant),
        ("BackendTlsPolicies", ResourceKind::BackendTLSPolicy),
    ] {
        assert_eq!(ResourceKind::from_kind_name(name), Some(expected));
    }
}
