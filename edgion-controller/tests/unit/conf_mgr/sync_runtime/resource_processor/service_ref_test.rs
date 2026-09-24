use super::*;
use edgion_resources::ResourceKind;

#[test]
fn service_events_merge_forwarding_and_mirror_roles_without_duplicates() {
    let manager = new_service_ref_manager();
    let route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "route".to_string());
    let plugin = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("ns".to_string()),
        "plugins".to_string(),
    );
    manager.add_ref("ns/service".to_string(), route.clone());
    manager.add_ref(mirror_service_key("ns", "service"), route.clone());
    manager.add_ref(mirror_service_key("ns", "service"), plugin.clone());

    assert_eq!(manager.get_refs("ns/service"), vec![route.clone()]);
    assert_eq!(
        forwarding_service_dependencies(&manager, &route.key()),
        vec!["ns/service".to_string()]
    );
    let refs: HashSet<_> = service_event_refs(&manager, "ns/service").into_iter().collect();
    assert_eq!(refs, HashSet::from([route, plugin]));
}
