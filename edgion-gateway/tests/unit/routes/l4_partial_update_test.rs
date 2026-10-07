use super::*;
use std::cell::RefCell;

fn listener(port: u16) -> ListenerKey {
    ListenerKey::new(([127, 0, 0, 1], port).into(), "TCP")
}

#[test]
fn failed_existing_add_clears_cache_and_rebuilds_old_listener() {
    let old = listener(9000);
    let cache = RefCell::new(HashMap::from([("route".to_string(), Arc::new(old.clone()))]));
    let rebuilt = RefCell::new(HashSet::new());
    apply_l4_partial_update(
        "TCPRoute",
        HashMap::from([("route".into(), ())]),
        HashMap::new(),
        HashSet::new(),
        |key| cache.borrow().get(key).cloned(),
        |key| vec![(**key).clone()],
        |key| {
            cache.borrow_mut().remove(key);
        },
        |_| {
            assert!(cache.borrow().is_empty(), "vacate before failed initialization");
            Err("invalid source".into())
        },
        |affected| *rebuilt.borrow_mut() = affected.clone(),
    );
    assert!(cache.borrow().is_empty());
    assert_eq!(*rebuilt.borrow(), HashSet::from([old]));
}

#[test]
fn overlapping_buckets_preserve_precollection_and_remove_update_add_order() {
    let old = listener(9000);
    let updated = listener(9001);
    let added = listener(9002);
    let cache = RefCell::new(HashMap::from([("route".to_string(), Arc::new(old.clone()))]));
    let events = RefCell::new(Vec::new());
    let rebuilt = RefCell::new(HashSet::new());
    apply_l4_partial_update(
        "TCPRoute",
        HashMap::from([("route".into(), added.clone())]),
        HashMap::from([("route".into(), updated.clone())]),
        HashSet::from(["route".into()]),
        |key| {
            events.borrow_mut().push("read");
            cache.borrow().get(key).cloned()
        },
        |key| vec![(**key).clone()],
        |key| {
            events.borrow_mut().push("remove");
            cache.borrow_mut().remove(key);
        },
        |key| {
            assert!(cache.borrow().is_empty());
            events.borrow_mut().push(if key == updated { "update" } else { "add" });
            cache.borrow_mut().insert("route".into(), Arc::new(key.clone()));
            Ok(vec![key])
        },
        |affected| *rebuilt.borrow_mut() = affected.clone(),
    );
    assert_eq!(
        *events.borrow(),
        ["read", "read", "read", "remove", "remove", "update", "remove", "add"]
    );
    assert_eq!(cache.borrow()["route"].as_ref(), &added);
    assert_eq!(*rebuilt.borrow(), HashSet::from([old, updated, added]));
}
