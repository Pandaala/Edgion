use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
use edgion_resources::ResourceKind;
use std::collections::HashSet;

#[test]
fn test_listener_finds_affected_resources() {
    let runtime = Arc::new(ControllerRuntime::new());
    let manager = &runtime.cross_ns_ref_mgr;

    manager.clear();

    let route = ResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());
    manager.add_ref("backend".to_string(), route);

    let _event = ReferenceGrantChangedEvent {
        affected_namespaces: {
            let mut set = HashSet::new();
            set.insert("backend".to_string());
            set
        },
    };

    let refs = manager.get_refs("backend");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].resource_key(), "app/my-route");
    assert_eq!(refs[0].kind_str(), "HTTPRoute");

    manager.clear();
}
