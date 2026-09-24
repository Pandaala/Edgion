use super::*;

#[test]
fn floor_is_ceil() {
    assert_eq!(degrade_floor(50, 10), 5); // 50% of 10
    assert_eq!(degrade_floor(50, 3), 2); // ceil(1.5)
    assert_eq!(degrade_floor(100, 10), 10); // require all healthy
    assert_eq!(degrade_floor(1, 1), 1);
}

#[test]
fn resolve_prefers_override_then_global() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_degrade_threshold_store();
    configure_lb_degrade_default(30);
    store.set_service_threshold("default/a", Some(70));
    assert_eq!(resolve_degrade_threshold("default/a"), 70);
    assert_eq!(resolve_degrade_threshold("default/no-override"), 30);
    store.set_service_threshold("default/a", None);
    assert_eq!(resolve_degrade_threshold("default/a"), 30);
}

#[test]
fn global_default_clamps_invalid() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(200);
    assert_eq!(get_lb_degrade_default(), 0);
}

#[test]
fn test_state_restores_default_and_all_overrides_on_unwind() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_degrade_threshold_store();
    configure_lb_degrade_default(25);
    store.set_service_threshold("guard/retained", Some(40));
    store.set_service_threshold("guard/removed", Some(60));
    let snapshot = DegradeTestState::capture();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _restore = snapshot;
        configure_lb_degrade_default(90);
        store.set_service_threshold("guard/retained", Some(80));
        store.set_service_threshold("guard/removed", None);
        store.set_service_threshold("guard/added", Some(100));
        panic!("exercise unwind restoration");
    }));
    assert!(result.is_err());
    assert_eq!(get_lb_degrade_default(), 25);
    assert_eq!(store.get_override("guard/retained"), Some(40));
    assert_eq!(store.get_override("guard/removed"), Some(60));
    assert_eq!(store.get_override("guard/added"), None);
}
