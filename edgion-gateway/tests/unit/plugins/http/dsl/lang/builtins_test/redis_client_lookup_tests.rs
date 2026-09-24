use super::resolve_redis_client_with;
use crate::plugins::http::dsl::runtime::DslRuntime;
use crate::plugins::runtime::PluginLog;
use crate::test_allocation_counter::count_allocation_stats;
use std::collections::HashMap;

#[test]
fn warm_alias_lookup_borrows_the_stored_link_key_without_allocating() {
    let runtime = DslRuntime {
        redis_links: HashMap::from([("cache".into(), "default/redis".into())]),
        ..Default::default()
    };
    let stored_key = runtime.redis_links.get("cache").expect("test alias must exist");
    let stored_key_ptr = stored_key.as_ptr();
    let mut log = PluginLog::new("test");

    let (result, allocations) = count_allocation_stats(|| {
        resolve_redis_client_with("cache", &runtime, usize::MAX, &mut log, |key: &str| {
            assert_eq!(key, "default/redis");
            assert_eq!(key.as_ptr(), stored_key_ptr);
            Some(())
        })
    });

    assert!(result.is_ok());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}
