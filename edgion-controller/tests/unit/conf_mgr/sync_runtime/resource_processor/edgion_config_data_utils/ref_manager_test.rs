use super::*;

#[test]
fn owner_retargeting_retires_empty_buckets_and_preserves_shared_dependents() {
    let manager = EdgionConfigDataRefManager::new();
    let owner = "EdgionPlugins:default/policy";
    let sibling = "Gateway:default/gateway";
    manager.add_ref("default/shared".into(), owner.into());
    manager.add_ref("default/shared".into(), sibling.into());
    for index in 0..128 {
        manager.clear_refs_from(owner);
        let target = format!("default/missing-{index}");
        manager.add_ref(target.clone(), owner.into());
        manager.add_ref(target.clone(), owner.into());
        let entries = manager.refs.read();
        assert_eq!(entries.len(), 2, "historical empty buckets must be removed");
        assert_eq!(entries[&target].len(), 1, "duplicate add stays idempotent");
        assert_eq!(entries["default/shared"], HashSet::from([sibling.to_string()]));
        if index > 0 {
            assert!(!entries.contains_key(&format!("default/missing-{}", index - 1)));
        }
    }
    manager.clear_refs_from(owner);
    manager.clear_refs_from(owner);
    assert_eq!(manager.refs.read().len(), 1);
    assert_eq!(manager.get_refs("default/shared"), [sibling]);
    manager.clear_refs_from(sibling);
    assert!(manager.refs.read().is_empty());
    manager.add_ref("default/new".into(), owner.into());
    manager.clear_all();
    assert!(manager.refs.read().is_empty());
}
