use super::*;

fn make_ref(kind: ResourceKind, ns: &str, name: &str) -> ResourceRef {
    ResourceRef::new(kind, Some(ns.to_string()), name.to_string())
}

// -- ResourceRef tests --------------------------------------------------

#[test]
fn test_resource_ref_key() {
    let r = make_ref(ResourceKind::EdgionTls, "default", "my-tls");
    assert_eq!(r.key(), "EdgionTls/default/my-tls");
    assert_eq!(r.resource_key(), "default/my-tls");
    assert_eq!(r.kind_str(), "EdgionTls");

    let r2 = ResourceRef::new(ResourceKind::EdgionTls, None, "cluster-tls".to_string());
    assert_eq!(r2.key(), "EdgionTls//cluster-tls");
    assert_eq!(r2.resource_key(), "cluster-tls");
}

#[test]
fn test_resource_ref_from_key() {
    let r = ResourceRef::from_key("EdgionTls/default/my-tls").unwrap();
    assert_eq!(r.kind, ResourceKind::EdgionTls);
    assert_eq!(r.namespace, Some("default".to_string()));
    assert_eq!(r.name, "my-tls");

    let r2 = ResourceRef::from_key("EdgionTls//cluster-tls").unwrap();
    assert_eq!(r2.namespace, None);
    assert_eq!(r2.name, "cluster-tls");

    assert!(ResourceRef::from_key("bad").is_none());
}

// -- BidirectionalRefManager tests --------------------------------------

#[test]
fn test_add_and_get_ref() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

    mgr.add_ref("default/my-cert".to_string(), resource.clone());

    let refs = mgr.get_refs("default/my-cert");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0], resource);

    let deps = mgr.get_dependencies(&resource.ref_key());
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0], "default/my-cert");
}

#[test]
fn test_remove_ref() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

    mgr.add_ref("default/my-cert".to_string(), resource.clone());
    mgr.remove_ref("default/my-cert", &resource);

    assert!(mgr.get_refs("default/my-cert").is_empty());
    assert!(mgr.get_dependencies(&resource.ref_key()).is_empty());
}

#[test]
fn test_clear_value_refs() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

    mgr.add_ref("default/cert1".to_string(), resource.clone());
    mgr.add_ref("default/cert2".to_string(), resource.clone());

    let cleared = mgr.clear_value_refs(&resource);
    assert_eq!(cleared.len(), 2);
    assert!(cleared.contains(&"default/cert1".to_string()));
    assert!(cleared.contains(&"default/cert2".to_string()));

    assert!(mgr.get_refs("default/cert1").is_empty());
    assert!(mgr.get_refs("default/cert2").is_empty());
}

#[test]
fn test_clear_source_refs() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let r1 = make_ref(ResourceKind::EdgionTls, "default", "tls1");
    let r2 = make_ref(ResourceKind::EdgionTls, "default", "tls2");

    mgr.add_ref("default/my-cert".to_string(), r1.clone());
    mgr.add_ref("default/my-cert".to_string(), r2.clone());
    // r1 also depends on a second cert; its reverse entry must be partially trimmed
    mgr.add_ref("default/other-cert".to_string(), r1.clone());

    let cleared = mgr.clear_source_refs("default/my-cert");
    assert_eq!(cleared.len(), 2);
    assert!(cleared.contains(&r1));
    assert!(cleared.contains(&r2));

    // forward entry removed
    assert!(mgr.get_refs("default/my-cert").is_empty());

    // r1's reverse entry still has "other-cert"; r2's reverse entry gone
    let r1_deps = mgr.get_dependencies(&r1.ref_key());
    assert_eq!(r1_deps, vec!["default/other-cert".to_string()]);
    assert!(mgr.get_dependencies(&r2.ref_key()).is_empty());

    // idempotent
    let again = mgr.clear_source_refs("default/my-cert");
    assert!(again.is_empty());
}

#[test]
fn test_multiple_values_same_source() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let r1 = make_ref(ResourceKind::EdgionTls, "default", "tls1");
    let r2 = make_ref(ResourceKind::EdgionTls, "default", "tls2");

    mgr.add_ref("default/my-cert".to_string(), r1);
    mgr.add_ref("default/my-cert".to_string(), r2);

    assert_eq!(mgr.get_refs("default/my-cert").len(), 2);
}

#[test]
fn test_idempotent_add() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let resource = make_ref(ResourceKind::EdgionTls, "default", "my-tls");

    mgr.add_ref("default/my-cert".to_string(), resource.clone());
    mgr.add_ref("default/my-cert".to_string(), resource.clone());
    mgr.add_ref("default/my-cert".to_string(), resource.clone());

    assert_eq!(mgr.get_refs("default/my-cert").len(), 1);
}

#[test]
fn test_all_source_keys() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let resource = make_ref(ResourceKind::HTTPRoute, "app", "route1");

    mgr.add_ref("backend1".to_string(), resource.clone());
    mgr.add_ref("backend2".to_string(), resource);

    let keys = mgr.all_source_keys();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&"backend1".to_string()));
    assert!(keys.contains(&"backend2".to_string()));
}

#[test]
fn test_stats() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let r1 = make_ref(ResourceKind::HTTPRoute, "app1", "route1");
    let r2 = make_ref(ResourceKind::HTTPRoute, "app2", "route2");

    mgr.add_ref("backend1".to_string(), r1.clone());
    mgr.add_ref("backend2".to_string(), r1);
    mgr.add_ref("backend1".to_string(), r2);

    let stats = mgr.stats();
    assert_eq!(stats.source_count, 2);
    assert_eq!(stats.value_count, 2);
    assert_eq!(stats.total_references, 3);
}

#[test]
fn test_clear() {
    let mgr = BidirectionalRefManager::<ResourceRef>::with_component("test");
    let resource = make_ref(ResourceKind::EdgionTls, "default", "tls1");
    mgr.add_ref("s1".to_string(), resource);

    assert_eq!(mgr.stats().source_count, 1);
    mgr.clear();
    assert_eq!(mgr.stats().source_count, 0);
    assert_eq!(mgr.stats().value_count, 0);
}

/// Concurrent stress test: simulates multiple ResourceController workers doing
/// the standard handler pattern (clear_value_refs → add_ref) concurrently.
///
/// With two separate RwLocks and reversed acquisition order between
/// `add_ref` (refs→deps) and `clear_value_refs` (deps→refs), this test
/// will deadlock. After merging into a single lock it must complete promptly.
#[test]
fn test_concurrent_clear_and_readd_no_deadlock() {
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    let mgr = Arc::new(BidirectionalRefManager::<ResourceRef>::new());

    // Pre-populate so there is something to clear immediately
    for i in 0..5u32 {
        let r = make_ref(ResourceKind::HTTPRoute, "ns", &format!("r{i}"));
        mgr.add_ref(format!("s{}", i % 3), r);
    }

    let (tx, rx) = mpsc::channel::<()>();

    // 4 threads each simulating a handler reconcile loop:
    //   clear old refs → add new refs   (high-contention, small key space)
    for t in 0..4u32 {
        let mgr = mgr.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            for i in 0..500u32 {
                let resource = make_ref(ResourceKind::HTTPRoute, "ns", &format!("r{}", (t + i) % 5));
                mgr.clear_value_refs(&resource);
                mgr.add_ref(format!("s{}", i % 3), resource);
            }
            let _ = tx.send(());
        });
    }
    drop(tx); // drop sender so recv_timeout can detect all done

    // All 4 threads must finish within 5 seconds total.
    // With the buggy dual-lock ordering this test deadlocks here.
    let deadline = Duration::from_secs(5);
    let start = std::time::Instant::now();
    let mut finished = 0;
    while finished < 4 {
        let remaining = deadline.saturating_sub(std::time::Instant::now().saturating_duration_since(start));
        match rx.recv_timeout(remaining) {
            Ok(()) => finished += 1,
            Err(_) => panic!(
                "Deadlock detected: concurrent add_ref / clear_value_refs \
                     did not complete within {:?}",
                deadline
            ),
        }
    }
}
