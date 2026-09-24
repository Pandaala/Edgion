use super::*;
use edgion_resources::ResourceKind;

#[test]
fn test_add_and_get_ref() {
    let manager = new_secret_ref_manager();
    let resource = ResourceRef::new(
        ResourceKind::EdgionTls,
        Some("default".to_string()),
        "my-tls".to_string(),
    );

    manager.add_ref("default/my-cert".to_string(), resource.clone());

    let refs = manager.get_refs("default/my-cert");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0], resource);

    let deps = manager.get_dependencies(&resource.key());
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0], "default/my-cert");
}

#[test]
fn test_clear_value_refs() {
    let manager = new_secret_ref_manager();
    let resource = ResourceRef::new(
        ResourceKind::EdgionTls,
        Some("default".to_string()),
        "my-tls".to_string(),
    );

    manager.add_ref("default/cert1".to_string(), resource.clone());
    manager.add_ref("default/cert2".to_string(), resource.clone());

    let cleared = manager.clear_value_refs(&resource);
    assert_eq!(cleared.len(), 2);
    assert!(cleared.contains(&"default/cert1".to_string()));
    assert!(cleared.contains(&"default/cert2".to_string()));

    assert!(manager.get_refs("default/cert1").is_empty());
    assert!(manager.get_refs("default/cert2").is_empty());
}

#[test]
fn test_idempotent_add() {
    let manager = new_secret_ref_manager();
    let resource = ResourceRef::new(
        ResourceKind::EdgionTls,
        Some("default".to_string()),
        "my-tls".to_string(),
    );

    manager.add_ref("default/my-cert".to_string(), resource.clone());
    manager.add_ref("default/my-cert".to_string(), resource.clone());
    manager.add_ref("default/my-cert".to_string(), resource.clone());

    assert_eq!(manager.get_refs("default/my-cert").len(), 1);
}
