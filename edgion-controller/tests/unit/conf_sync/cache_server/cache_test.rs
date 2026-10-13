use super::*;
use edgion_common::conf_sync::traits::ResourceChange;
use edgion_common::conf_sync::CacheEventDispatch;
use edgion_resources::{ResourceKind, ResourceMeta};
use kube::api::ObjectMeta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestResource {
    name: String,
    version: u64,
    metadata: ObjectMeta,
}

impl PartialEq for TestResource {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.version == other.version
    }
}

impl Eq for TestResource {}

impl ResourceMeta for TestResource {
    fn get_version(&self) -> u64 {
        self.version
    }
    fn get_sync_version(&self) -> u64 {
        0
    }
    fn resource_kind() -> ResourceKind {
        ResourceKind::Unspecified
    }
    fn kind_name() -> &'static str {
        "TestResource"
    }
    fn key_name(&self) -> String {
        "test".to_string()
    }
}

impl kube::Resource for TestResource {
    type DynamicType = ();
    type Scope = kube::core::ClusterResourceScope;

    fn kind(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "TestResource".into()
    }

    fn group(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "test.example.com".into()
    }

    fn version(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "v1".into()
    }

    fn plural(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "testresources".into()
    }

    fn meta(&self) -> &ObjectMeta {
        &self.metadata
    }

    fn meta_mut(&mut self) -> &mut ObjectMeta {
        &mut self.metadata
    }
}

fn make_resource(name: &str, version: u64) -> TestResource {
    TestResource {
        name: name.to_string(),
        version,
        metadata: ObjectMeta {
            name: Some("test-resource".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn event_add_stores_resource_and_updates_version() {
    let cache = ServerCache::<TestResource>::new(10);
    let resource = make_resource("foo", 1);

    cache.apply_change(ResourceChange::EventAdd, resource.clone(), None);

    let snapshot = cache.list_owned();
    assert_eq!(snapshot.data.len(), 1);
    assert_eq!(snapshot.data[0], resource);
    // sync_version is independently generated; verify it's set.
    assert!(snapshot.sync_version > 0);
}

#[tokio::test]
async fn event_update_replaces_existing_resource() {
    let cache = ServerCache::<TestResource>::new(10);
    let original = make_resource("foo", 1);
    let updated = make_resource("foo-updated", 2);

    CacheEventDispatch::set_ready(&cache);

    cache.apply_change(ResourceChange::EventAdd, original.clone(), None);
    cache.apply_change(ResourceChange::EventUpdate, updated.clone(), None);

    let snapshot = cache.list_owned();
    assert_eq!(snapshot.data.len(), 1);
    assert_eq!(snapshot.data[0], updated);
    // sync_version is independently generated; verify it's set.
    assert!(snapshot.sync_version > 0);
}

#[tokio::test]
async fn event_delete_removes_resource() {
    let cache = ServerCache::<TestResource>::new(10);
    let resource = make_resource("foo", 42);
    let resource_delete = make_resource("foo", 43);

    CacheEventDispatch::set_ready(&cache);

    cache.apply_change(ResourceChange::EventAdd, resource.clone(), None);
    cache.apply_change(ResourceChange::EventDelete, resource_delete.clone(), None);

    let snapshot = cache.list_owned();
    assert!(snapshot.data.is_empty());
    // sync_version is independently generated; verify it's set.
    assert!(snapshot.sync_version > 0);
}

/// Rapid sequential apply_change calls must leave the cache in the state of the
/// LAST call, not some arbitrary scheduler-determined order.
#[tokio::test]
async fn rapid_updates_preserve_latest_state() {
    let cache = ServerCache::<TestResource>::new(200);
    CacheEventDispatch::set_ready(&cache);

    // Issue 50 rapid updates sequentially.
    let n: u64 = 50;
    for i in 1..=n {
        let r = make_resource(&format!("state-{}", i), i);
        cache.apply_change(ResourceChange::EventUpdate, r, None);
    }

    // Final snapshot must reflect the last update
    let snapshot = cache.list_owned();
    assert_eq!(snapshot.data.len(), 1, "only one item in the store");
    assert_eq!(
        snapshot.data[0].name,
        format!("state-{}", n),
        "cache must contain the last (highest-versioned) state after rapid updates"
    );
    assert!(snapshot.sync_version > 0);
}

/// Concurrent `apply_change` calls must never lose an event, and the circular queue must
/// stay monotonic so `get_events_from_sync_version`'s backward scan can resume a watch.
///
/// Regression for the cache-residue incident: `sync_version` used to be drawn before the
/// store lock was taken, so a thread could be overtaken between drawing its number and
/// applying it. The store's then-kind-wide stale guard saw the overtaken thread's lower
/// number and silently discarded a legitimate event -- a deleted Gateway stayed in the
/// data plane's cache forever.
///
/// **Concurrency is the discriminator, not `None` vs `Some`.** A sequential test cannot
/// detect this bug in either direction: the old code drew the version a few instructions
/// before applying it, on the same thread, so sequential calls came out monotonic under
/// the old code too. (Verified: an earlier sequential version of this test passed against
/// the buggy commit.) The bug needs a real overlap -- thread A draws, thread B draws AND
/// applies, thread A applies -- which only threads can produce. The old suite missed this
/// bug for exactly the same reason.
///
/// Against the buggy code this fails with high probability (the window between drawing
/// and applying spans a `tracing::info!`); against the fix it cannot fail, because the
/// version is drawn under the same lock that applies it.
#[test]
fn concurrent_apply_change_never_drops_events_and_stays_monotonic() {
    use std::sync::{Arc, Barrier};

    const THREADS: usize = 8;
    const ROUNDS: usize = 16;

    for round in 0..ROUNDS {
        let cache = Arc::new(ServerCache::<TestResource>::new(200));
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    // Maximise the overlap: every thread is released at once.
                    barrier.wait();
                    // Distinct key per thread -- these are unrelated resources, so no
                    // event may suppress any other. That is the whole point.
                    cache.apply_change(ResourceChange::EventAdd, make_resource(&format!("r{i}"), 1), None);
                })
            })
            .collect();

        for h in handles {
            h.join().expect("worker thread panicked");
        }

        let (_, events_opt) = cache
            .store
            .read()
            .get_events_from_sync_version(0)
            .expect("watch from 0 must succeed");
        let versions: Vec<u64> = events_opt
            .expect("expected events")
            .iter()
            .map(|e| e.sync_version)
            .collect();

        assert_eq!(
            versions.len(),
            THREADS,
            "round {round}: {} of {THREADS} concurrent events were dropped; versions={versions:?}",
            THREADS - versions.len()
        );
        assert!(
            versions.windows(2).all(|w| w[1] > w[0]),
            "round {round}: queue must be monotonic in apply order, got {versions:?}"
        );
    }
}

#[tokio::test]
async fn current_version_tracks_store_version() {
    let cache = ServerCache::<TestResource>::new(10);
    assert_eq!(WatchObj::current_version(&cache), 0);

    cache.apply_change(ResourceChange::EventAdd, make_resource("foo", 1), Some(42));
    assert_eq!(WatchObj::current_version(&cache), 42);

    // Contract: same store list_json serializes.
    let (_json, v) = cache.list_json().unwrap();
    assert_eq!(v, 42);
}

#[tokio::test]
async fn scan_wait_boundary_retains_one_broadcast_for_all_unpolled_watchers() {
    let cache = ServerCache::<TestResource>::new(10);
    CacheEventDispatch::set_ready(&cache);
    let (first, initial) = ServerCache::prepare_watch_scan(&cache.store, &cache.notify, 0);
    assert!(matches!(initial, Ok((0, None))));
    let (second, initial) = ServerCache::prepare_watch_scan(&cache.store, &cache.notify, 0);
    assert!(matches!(initial, Ok((0, None))));

    // Publish exactly once after the empty scan and before either future is polled.
    let resource = make_resource("single-update", 1);
    cache.apply_change(ResourceChange::EventAdd, resource.clone(), Some(1));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        tokio::join!(first, second);
    })
    .await
    .expect("both pre-scan futures must retain the single broadcast");

    let (_next_notification, result) = ServerCache::prepare_watch_scan(&cache.store, &cache.notify, 0);
    let (version, events) = result.unwrap();
    assert_eq!(version, 1);
    let events = events.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, resource);
    assert_eq!(events[0].sync_version, 1);
}

#[tokio::test]
async fn spawned_watchers_deliver_one_event_and_release_closed_receivers() {
    let cache = ServerCache::<TestResource>::new(10);
    CacheEventDispatch::set_ready(&cache);
    let baseline_store_owners = Arc::strong_count(&cache.store);
    let baseline_notify_owners = Arc::strong_count(&cache.notify);
    let mut first = cache.watch("first".into(), "first".into(), 0);
    let mut second = cache.watch("second".into(), "second".into(), 0);
    let resource = make_resource("broadcast", 1);
    cache.apply_change(ResourceChange::EventAdd, resource.clone(), Some(1));
    for receiver in [&mut first, &mut second] {
        let response = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(response.err.is_none());
        assert_eq!(response.sync_version, 1);
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].data, resource);
    }
    // No further event or generation change is needed to release either task.
    drop(first);
    drop(second);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while Arc::strong_count(&cache.store) != baseline_store_owners
            || Arc::strong_count(&cache.notify) != baseline_notify_owners
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closed receivers must release watcher-owned store and Notify handles");
}
