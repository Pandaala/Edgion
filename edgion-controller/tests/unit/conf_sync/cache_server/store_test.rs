use super::*;
use kube::api::ObjectMeta;

// Test resource type that implements Resource trait
use edgion_resources::{ResourceKind, ResourceMeta};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestResource {
    name: String,
    namespace: Option<String>,
    metadata: ObjectMeta,
}

impl PartialEq for TestResource {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.namespace == other.namespace
    }
}

impl Eq for TestResource {}

impl ResourceMeta for TestResource {
    fn get_version(&self) -> u64 {
        0
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
        "TestResource".to_string()
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

impl TestResource {
    fn new(name: &str, namespace: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: namespace.map(|s| s.to_string()),
                ..Default::default()
            },
        }
    }

    /// Create a resource whose storage key (`metadata.name`) stays stable,
    /// while `name` acts as test payload for overwrite assertions.
    fn with_key_and_value(key: &str, value: &str, namespace: Option<&str>) -> Self {
        let mut r = Self::new(key, namespace);
        r.name = value.to_string();
        r
    }
}

fn make_store() -> EventStore<TestResource> {
    EventStore::new(10)
}

#[test]
fn empty_store_returns_no_events() {
    let store = make_store();

    let (current_version, events) = store.get_events_from_sync_version(0).unwrap();

    assert_eq!(current_version, 0);
    assert!(events.is_none());
}

#[test]
fn apply_event_adds_data_and_updates_version() {
    let mut store = make_store();

    let resource = TestResource::new("alpha", Some("default"));
    store.apply_event(EventType::Add, resource.clone(), 1);

    let (snapshot, version) = store.snapshot_owned();
    assert_eq!(version, 1);
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot.contains(&resource));

    let (current_version, events_opt) = store.get_events_from_sync_version(0).unwrap();
    let events = events_opt.expect("expected events");
    assert_eq!(current_version, 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sync_version, 1);
    assert_eq!(events[0].data, resource);
    assert!(matches!(events[0].event_type, EventType::Add));
}

#[test]
fn get_events_filters_by_requested_version() {
    let mut store = make_store();

    let alpha = TestResource::new("alpha", Some("default"));
    let beta = TestResource::new("beta", Some("default"));
    store.apply_event(EventType::Add, alpha, 1);
    store.apply_event(EventType::Update, beta.clone(), 2);

    let (_, events_opt) = store.get_events_from_sync_version(1).unwrap();
    let events = events_opt.expect("expected events for version > 1");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sync_version, 2);
    assert_eq!(events[0].data, beta);
    assert!(matches!(events[0].event_type, EventType::Update));

    let (_, events_opt) = store.get_events_from_sync_version(2).unwrap();
    assert!(
        events_opt.is_none(),
        "no events expected when requesting current version"
    );
}

#[test]
fn zero_cursor_expires_only_after_history_is_evicted() {
    let mut store = make_store();
    let (snapshot, cursor) = store.snapshot_owned();
    assert!(snapshot.is_empty());
    assert_eq!(cursor, 0);

    for version in 1..=10 {
        store.apply_event(
            EventType::Add,
            TestResource::new(&format!("resource-{version}"), Some("default")),
            version,
        );
    }
    let (version, events) = store.get_events_from_sync_version(cursor).unwrap();
    assert_eq!(version, 10);
    assert_eq!(events.unwrap().len(), 10);

    store.apply_event(EventType::Add, TestResource::new("resource-11", Some("default")), 11);
    assert_eq!(
        store.get_events_from_sync_version(cursor).unwrap_err(),
        WATCH_ERR_TOO_OLD_VERSION
    );
    // A cursor equal to the evicted version has already consumed that event.
    let (version, events) = store.get_events_from_sync_version(1).unwrap();
    assert_eq!(version, 11);
    assert_eq!(
        events
            .unwrap()
            .iter()
            .map(|event| event.sync_version)
            .collect::<Vec<_>>(),
        (2..=11).collect::<Vec<_>>()
    );
    let (snapshot, version) = store.snapshot_owned();
    assert_eq!(version, 11);
    assert_eq!(snapshot.len(), 11);
}

#[test]
fn stale_version_error_when_events_expired() {
    let mut store: EventStore<TestResource> = EventStore::new(50);

    for version in 1..=60 {
        let resource = TestResource::new(&format!("v{}", version), Some("default"));
        store.apply_event(EventType::Add, resource, version);
    }

    let err = store
        .get_events_from_sync_version(5)
        .expect_err("expected stale version error");
    assert_eq!(err, "TooOldVersion");

    let (current_version, events_opt) = store.get_events_from_sync_version(55).unwrap();
    let events = events_opt.expect("expected events after catching up");
    assert_eq!(current_version, 60);
    let versions: Vec<u64> = events.iter().map(|ev| ev.sync_version).collect();
    assert_eq!(versions, vec![56, 57, 58, 59, 60]);
}

#[test]
fn multiple_wraps_over_capacity() {
    let mut store: EventStore<TestResource> = EventStore::new(50);

    for version in 1..=120 {
        let resource = TestResource::new(&format!("value-{}", version), Some("default"));
        store.apply_event(EventType::Add, resource, version);
    }

    let (current_version, events_opt) = store.get_events_from_sync_version(110).unwrap();
    let events = events_opt.expect("expected events after wrap");
    assert_eq!(current_version, 120);
    let versions: Vec<u64> = events.iter().map(|ev| ev.sync_version).collect();
    assert_eq!(versions, (111..=120).collect::<Vec<_>>());

    for (offset, event) in events.iter().enumerate() {
        assert_eq!(event.data.name, format!("value-{}", 111 + offset as u64));
    }

    let err = store
        .get_events_from_sync_version(10)
        .expect_err("versions older than expire_sync_version should error");
    assert_eq!(err, "TooOldVersion");
}

#[test]
fn version_unexpect_error_when_requesting_future_version() {
    let mut store = make_store();

    let resource = TestResource::new("alpha", Some("default"));
    store.apply_event(EventType::Add, resource, 1);

    let err = store
        .get_events_from_sync_version(99)
        .expect_err("requesting future version should error");
    assert_eq!(err, "VersionUnexpect");
}

#[test]
fn duplicate_resource_replaces_existing() {
    let mut store = make_store();

    // Add same resource twice with different versions
    let resource1 = TestResource::new("test-route", Some("default"));
    let resource2 = TestResource::new("test-route", Some("default"));

    store.apply_event(EventType::Add, resource1.clone(), 1);
    store.apply_event(EventType::Add, resource2.clone(), 2);

    // Should only have one entry (the latest)
    let (snapshot, version) = store.snapshot_owned();
    assert_eq!(version, 2);
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].name, "test-route");
}

/// An out-of-order explicit `sv` must be ADMITTED (with a warning), never dropped.
///
/// The store cannot repair ordering its caller broke -- the circular queue is in
/// insertion order -- but it must not destroy data over it. Silently dropping a
/// legitimate event is exactly the cache-residue incident this guard was rescoped to
/// fix. The correct response to a broken invariant is a loud warning plus the data.
#[test]
fn out_of_order_explicit_version_is_admitted_not_dropped() {
    let mut store = make_store();

    store.apply_event(EventType::Add, TestResource::new("alpha", Some("default")), 100);
    store.apply_event(EventType::Add, TestResource::new("beta", Some("default")), 200);

    // A LOWER version for a DIFFERENT key: the caller broke queue monotonicity.
    // `alpha`'s own last version is 100, so the per-key guard correctly allows this.
    store.apply_event(EventType::Delete, TestResource::new("alpha", Some("default")), 150);

    let (snapshot, _) = store.snapshot_owned();
    let names: Vec<&str> = snapshot.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["beta"],
        "alpha's delete carried a lower version than an unrelated resource's event, but it \
             is not stale for alpha -- it must be applied, not dropped"
    );
}

/// Regression: the stale-event guard must be scoped PER RESOURCE, not per kind.
///
/// Reproduces the 2026-07-17 integration failure (cache residue after wipe-all).
/// Real trace from the controller log:
///
///   .129972  process_delete Gateway/invalid-ca-cert-gw  sv=4589  (slow: clearing 2 secret refs)
///   .130127  process_delete Gateway/security-gateway    sv=4591  -> applied, store version = 4591
///   .130811  process_delete Gateway/invalid-ca-cert-gw  sv=4589  -> DROPPED as "stale"
///
/// Two unrelated Gateways. The second delete was dropped because a *different*
/// resource of the same kind had already pushed the store's single high-water
/// mark past it, so the Gateway never learned about the deletion and kept the
/// object in its cache forever.
#[test]
fn stale_guard_must_not_drop_events_for_a_different_resource() {
    let mut store = make_store();

    let slow = TestResource::new("invalid-ca-cert-gw", Some("edgion-test"));
    let fast = TestResource::new("security-gateway", Some("edgion-test"));

    store.apply_event(EventType::Add, slow.clone(), 4589);
    store.apply_event(EventType::Add, fast.clone(), 4590);

    // `fast` is deleted first and carries the higher version.
    store.apply_event(EventType::Delete, fast, 4591);

    // `slow`'s delete was allocated an EARLIER version but arrives LATER.
    // It concerns a different key, so it must still be applied.
    store.apply_event(EventType::Delete, slow, 4589);

    let (snapshot, _) = store.snapshot_owned();
    assert_eq!(
        snapshot.len(),
        0,
        "both resources were deleted; a delete for one key must not be \
             suppressed by a newer event for a different key. Residual: {:?}",
        snapshot.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
}

/// A stale event (lower sync_version) must not overwrite state written by a newer event.
#[test]
fn stale_event_does_not_overwrite_newer_state() {
    let mut store = make_store();

    // Apply the "newer" event first (simulates the scheduler running v2 before v1)
    // Use the same resource key to verify overwrite semantics on one object.
    let newer = TestResource::with_key_and_value("test-route", "new-value", Some("default"));
    store.apply_event(EventType::Update, newer.clone(), 10);

    // Now attempt to apply a stale event with a lower sync_version
    let stale = TestResource::with_key_and_value("test-route", "old-value", Some("default"));
    store.apply_event(EventType::Update, stale.clone(), 5);

    // The store must reflect the newer state, not the stale one
    let (snapshot, version) = store.snapshot_owned();
    assert_eq!(
        version, 10,
        "sync_version must stay at the newer value after a stale event is dropped"
    );
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot[0].name, "new-value",
        "stale event must not overwrite newer state"
    );

    // The stale event must also not appear in the event queue
    let (_, events_opt) = store.get_events_from_sync_version(0).unwrap();
    let events = events_opt.expect("expected events");
    assert_eq!(events.len(), 1, "only the newer event should be in the queue");
    assert_eq!(events[0].sync_version, 10);
    assert_eq!(events[0].data.name, "new-value");
}

/// When events arrive out of order (v2 before v1), the snapshot and version must
/// reflect the highest-versioned write, not the last wall-clock write.
#[test]
fn out_of_order_apply_sequence_preserves_latest_data() {
    let mut store = make_store();

    // Simulate a scenario where v1 arrives first (normal order)
    // Keep the same key across versions so data replacement is exercised.
    let v1 = TestResource::with_key_and_value("test-route", "state-v1", Some("default"));
    store.apply_event(EventType::Add, v1.clone(), 1);

    // v3 arrives next (out of order - scheduler executed it before v2)
    let v3 = TestResource::with_key_and_value("test-route", "state-v3", Some("default"));
    store.apply_event(EventType::Update, v3.clone(), 3);

    // v2 arrives last - must be dropped because version 3 is already committed
    let v2 = TestResource::with_key_and_value("test-route", "state-v2", Some("default"));
    store.apply_event(EventType::Update, v2.clone(), 2);

    let (snapshot, version) = store.snapshot_owned();
    assert_eq!(version, 3, "sync_version must be the highest seen");
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot[0].name, "state-v3",
        "data must reflect v3 (highest version), not the last-arriving v2"
    );

    // Event queue must contain v1 and v3 (v2 was dropped)
    let (_, events_opt) = store.get_events_from_sync_version(0).unwrap();
    let events = events_opt.expect("expected events");
    let versions: Vec<u64> = events.iter().map(|e| e.sync_version).collect();
    assert_eq!(versions, vec![1, 3], "only in-order events should be in the queue");
}

#[test]
fn sync_version_getter_tracks_applied_events() {
    let mut store = make_store();
    assert_eq!(store.sync_version(), 0);

    let resource = TestResource::new("alpha", Some("default"));
    store.apply_event(EventType::Add, resource, 7);
    assert_eq!(store.sync_version(), 7);

    store.clear();
    assert_eq!(store.sync_version(), 0);
}
