use super::types::{EventType, WatcherEvent};
use edgion_resources::{WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
use kube::{Resource, ResourceExt};
use serde::Serialize;
use std::collections::HashMap;

/// Result type for get_events_from_sync_version: (current_version, optional_events)
type EventsResult<T> = Result<(u64, Option<Vec<WatcherEvent<T>>>), String>;

/// Event storage - circular queue
pub struct EventStore<T> {
    capacity: usize,
    cache: Vec<Option<WatcherEvent<T>>>,
    start_index: usize,
    end_index: usize,
    sync_version: u64,
    expire_sync_version: u64,
    data: HashMap<String, T>, // Key: namespace/name or name (for cluster-scoped resources)
    /// Last applied sync_version per resource key, for the staleness guard in `apply_event`.
    ///
    /// Deliberately keyed per resource, NOT per kind: `sync_version` above is a kind-wide
    /// high-water mark, and comparing one resource's event against it discards legitimate
    /// events for unrelated resources.
    ///
    /// Entries are kept after Delete on purpose. Dropping the entry would let a stale
    /// post-delete Update resurrect the resource, which is the failure this guard exists to
    /// prevent.
    ///
    /// Growth: bounded by the distinct keys seen since **process start**, not since the last
    /// relink. `clear()` does reset this map, but its only non-test caller is the non-K8s
    /// config-center reload path (`conf_center/non_kube_common.rs`) -- in Kubernetes mode it
    /// never runs, and a gateway relink does not clear the server-side store. Steady-state
    /// clusters reuse resource names, so in practice this tracks live resource count. The
    /// exposure is generated names: `EndpointSlice` keys are `<svc>-<random>`, so churn from
    /// ephemeral namespaces accumulates entries (~100 B each) that are never reclaimed.
    /// Not urgent at realistic churn rates, but it is a real leak, not a self-limiting cache.
    last_version_by_key: HashMap<String, u64>,
}

impl<T> EventStore<T> {
    // init add do not apply any events
    pub fn init_add(&mut self, sync_version: u64, resource: T)
    where
        T: Resource,
    {
        let key = Self::resource_key(&resource);
        self.data.insert(key, resource);
        if sync_version > self.sync_version {
            self.sync_version = sync_version;
        }
    }

    /// Clear all data and reset to initial state
    /// Used during relink to clear stale data
    pub fn clear(&mut self)
    where
        T: Clone,
    {
        self.cache = vec![None; self.capacity];
        self.start_index = 0;
        self.end_index = 0;
        self.sync_version = 0;
        self.expire_sync_version = 0;
        self.data.clear();
        self.last_version_by_key.clear();
    }

    pub fn apply_event(&mut self, event_type: EventType, resource: T, sync_version: u64)
    where
        T: Clone + Resource,
    {
        let key = Self::resource_key(&resource);

        // Staleness guard, scoped PER KEY. Only a newer event for the SAME resource may
        // suppress this one. Scoping it per kind -- comparing against `self.sync_version`,
        // a kind-wide high-water mark -- discards legitimate events for unrelated resources:
        // a Gateway delete was thrown away because a different Gateway's delete had already
        // landed with a higher number, leaving the object in the data plane's cache forever.
        if let Some(&last) = self.last_version_by_key.get(&key) {
            if sync_version < last {
                tracing::warn!(
                    kind = std::any::type_name::<T>(),
                    event_type = ?event_type,
                    resource_name = %resource.name_any(),
                    resource_namespace = ?resource.namespace(),
                    incoming_sync_version = sync_version,
                    last_sync_version_for_key = last,
                    "Stale event dropped: a newer event for this key was already applied"
                );
                return;
            }
        }

        // Queue-monotonicity check. Versions are drawn under this store's write lock
        // (ServerCache::push_event), so apply order == allocation order and this is
        // unreachable. A hit means a caller passed an explicit `sv` allocated outside the
        // lock, and the circular queue's ordering invariant is now broken --
        // get_events_from_sync_version's backward scan stops at the first event whose
        // version is <= from_version, so a watcher would silently miss events.
        //
        // Warn; do NOT drop. The store cannot repair ordering its caller broke, but it must
        // not destroy data over it: silently dropping a legitimate event is the bug this
        // guard was rescoped to fix. An ordering violation deserves a scream, not a discard.
        if sync_version < self.sync_version {
            tracing::warn!(
                kind = std::any::type_name::<T>(),
                event_type = ?event_type,
                resource_name = %resource.name_any(),
                incoming_sync_version = sync_version,
                current_sync_version = self.sync_version,
                "Out-of-order event admitted: queue monotonicity violated, watch resumption may truncate"
            );
        }

        self.last_version_by_key.insert(key.clone(), sync_version);
        match event_type {
            EventType::Add | EventType::Update => {
                // Use namespace/name as key to ensure uniqueness
                // If resource already exists, it will be replaced (update)
                self.data.insert(key, resource.clone());
            }
            EventType::Delete => {
                // Remove by namespace/name key, not version
                self.data.remove(&key);
            }
        }

        if sync_version > self.sync_version {
            self.sync_version = sync_version;
        }

        let event = WatcherEvent {
            event_type,
            sync_version,
            data: resource,
        };

        self.mut_update(event);
    }

    /// Generate a unique key for a resource based on namespace and name
    fn resource_key(resource: &T) -> String
    where
        T: Resource,
    {
        let namespace = resource.namespace();
        let name = resource.name_any();

        if let Some(ns) = namespace {
            format!("{}/{}", ns, name)
        } else {
            name
        }
    }

    pub fn snapshot_owned(&self) -> (Vec<T>, u64)
    where
        T: Clone,
    {
        let data = self.data.values().cloned().collect();
        (data, self.sync_version)
    }

    /// Serialize data to JSON, collecting references rather than cloning values.
    /// Avoids the full `T`-clone that `snapshot_owned` performs.
    pub fn serialize_snapshot_json(&self) -> Result<(String, u64), String>
    where
        T: Serialize,
    {
        let values: Vec<&T> = self.data.values().collect();
        let json = serde_json::to_string(&values).map_err(|e| e.to_string())?;
        Ok((json, self.sync_version))
    }

    pub fn keys(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }

    /// Return the number of stored resources (O(1)).
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Current sync version (O(1) field read).
    pub fn sync_version(&self) -> u64 {
        self.sync_version
    }

    /// Get a single resource by key
    /// Key format: "namespace/name" for namespaced resources, or just "name" for cluster-scoped
    pub fn get_by_key(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        self.data.get(key).cloned()
    }
}

impl<T: Clone> EventStore<T> {
    pub fn new(capacity: usize) -> Self {
        let capacity = if capacity == 0 { 200 } else { capacity.max(10) };
        assert!(
            capacity > 5,
            "EventStore capacity must be greater than 5, got {capacity}"
        );

        Self {
            capacity,
            cache: vec![None; capacity],
            start_index: 0,
            end_index: 0,
            sync_version: 0,
            expire_sync_version: 0,
            data: HashMap::new(),
            last_version_by_key: HashMap::new(),
        }
    }

    /// Add new event to circular queue
    pub fn mut_update(&mut self, event: WatcherEvent<T>) {
        let index = self.end_index % self.capacity;

        if self.end_index - self.start_index >= self.capacity {
            if let Some(last_event) = self.cache[index].as_ref() {
                self.expire_sync_version = last_event.sync_version;
            }
            self.start_index += 1;
        }

        self.cache[index] = Some(event);
        self.end_index += 1;
    }

    /// Get events starting from specified version
    pub fn get_events_from_sync_version(&self, from_version: u64) -> EventsResult<T> {
        if from_version > self.sync_version {
            return Err(WATCH_ERR_VERSION_UNEXPECTED.to_owned());
        } else if from_version == self.sync_version {
            return Ok((self.sync_version, None));
        }

        if from_version != 0 && from_version < self.expire_sync_version {
            return Err(WATCH_ERR_TOO_OLD_VERSION.to_owned());
        }

        if self.capacity == 0 || self.end_index == self.start_index {
            return Ok((self.sync_version, None));
        }

        // Walk backward to find the earliest index whose version is > from_version.
        let mut start_scan = self.end_index;
        while start_scan > self.start_index {
            let idx = (start_scan - 1) % self.capacity;
            match self.cache[idx].as_ref() {
                Some(ev) if ev.sync_version > from_version => {
                    start_scan -= 1;
                }
                _ => break,
            }
        }

        let mut events = Vec::new();
        let mut loop_index = start_scan;
        while loop_index < self.end_index {
            let idx = loop_index % self.capacity;
            if let Some(ev) = self.cache[idx].as_ref() {
                if ev.sync_version > from_version {
                    events.push(ev.clone());
                }
            }
            loop_index += 1;
        }

        Ok((self.sync_version, Some(events)))
    }
}

#[cfg(test)]
mod tests {
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
}
