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

        if from_version < self.expire_sync_version {
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
#[path = "../../../tests/unit/conf_sync/cache_server/store_test.rs"]
mod tests;
