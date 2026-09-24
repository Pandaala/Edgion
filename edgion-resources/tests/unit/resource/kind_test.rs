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
