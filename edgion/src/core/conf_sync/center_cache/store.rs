use super::types::{EventType, WatcherEvent};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

/// Event storage - circular queue
pub struct EventStore<T> {
    capacity: usize,
    cache: Vec<Option<WatcherEvent<T>>>,
    start_index: usize,
    end_index: usize,
    resource_version: u64,
    expire_version: u64,
    data: HashMap<String, T>,
}

impl<T> EventStore<T> {
    /// Set current resource version
    pub fn set_current_version(&mut self, version: u64) {
        self.resource_version = version;
    }

    // init add do not apply any events
    pub fn init_add(&mut self, version: u64, resource: T) {
        self.data.insert(version.to_string(), resource);
        if version > self.resource_version {
            self.resource_version = version;
        }
    }

    pub fn apply_event(&mut self, event_type: EventType, resource: T, version: u64)
    where
        T: Clone,
    {
        match event_type {
            EventType::Add | EventType::Update => {
                self.data.insert(version.to_string(), resource.clone());
            }
            EventType::Delete => {
                self.data.remove(&version.to_string());
            }
        }

        if version > self.resource_version {
            self.resource_version = version;
        }

        let event = WatcherEvent {
            event_type,
            resource_version: version,
            data: resource,
        };

        self.mut_update(event);
    }

    pub fn snapshot_owned(&self) -> (Vec<T>, u64)
    where
        T: Clone,
    {
        let data = self.data.values().cloned().collect();
        (data, self.resource_version)
    }
}

impl<T: Clone> EventStore<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            cache: Vec::with_capacity(capacity + 1),
            start_index: 0,
            end_index: 0,
            resource_version: 0,
            expire_version: 0,
            data: HashMap::new(),
        }
    }

    /// Add new event to circular queue
    pub fn mut_update(&mut self, event: WatcherEvent<T>) {
        if (self.end_index >= self.capacity) {
            let index = self.end_index % self.capacity;
            if let Some(last_event) = self.cache.get(index).unwrap() {
                self.expire_version = last_event.resource_version;
            }
            self.cache[index] = Some(event);
            self.end_index += 1;
            self.start_index += 1;
        } else {
            self.cache[self.end_index] = Some(event);
            self.end_index += 1;
        }
    }

    /// Get events starting from specified version
    pub fn get_events_from_resource_version(
        &self,
        from_version: u64,
    ) -> Result<(u64, Vec<WatcherEvent<T>>), String> {
        if from_version != 0 && from_version < self.expire_version {
            return Err("failed".to_owned());
        }

        if self.cache.is_empty() {
            return Ok((0, Vec::new()));
        }

        let mut events = Vec::new();

        let mut loop_index = self.end_index;
        while loop_index >= self.start_index {
            if let Some(ev) = self.cache.get(loop_index % self.capacity).unwrap() {
                if ev.resource_version > from_version {
                    events.push(ev.clone())
                } else {
                    break;
                }
            } else {
                panic!("error no ev")
            }
            loop_index -= 1;
        }

        Ok((self.resource_version, events))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> EventStore<String> {
        EventStore::new(4)
    }

    #[test]
    fn empty_store_returns_no_events() {
        let store = make_store();

        let (current_version, events) = store.get_events_from_resource_version(0).unwrap();

        assert_eq!(current_version, 0);
        assert!(events.is_empty());
    }

    #[test]
    fn apply_event_adds_data_and_updates_version() {
        let mut store = make_store();

        store.apply_event(EventType::Add, "alpha".to_string(), 1);

        let (snapshot, version) = store.snapshot_owned();
        assert_eq!(version, 1);
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot.contains(&"alpha".to_string()));

        let (current_version, events) = store.get_events_from_resource_version(0).unwrap();
        assert_eq!(current_version, 1);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_version, 1);
        assert_eq!(events[0].data, "alpha");
        assert!(matches!(events[0].event_type, EventType::Add));
    }

    #[test]
    fn get_events_filters_by_requested_version() {
        let mut store = make_store();

        store.apply_event(EventType::Add, "alpha".to_string(), 1);
        store.apply_event(EventType::Update, "beta".to_string(), 2);

        let (_, events) = store.get_events_from_resource_version(1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_version, 2);
        assert_eq!(events[0].data, "beta");
        assert!(matches!(events[0].event_type, EventType::Update));

        let (_, events) = store.get_events_from_resource_version(2).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn stale_version_error_when_events_expired() {
        let mut store: EventStore<String> = EventStore::new(2);

        store.apply_event(EventType::Add, "v1".to_string(), 1);
        store.apply_event(EventType::Add, "v2".to_string(), 2);
        store.apply_event(EventType::Add, "v3".to_string(), 3);

        let err = store
            .get_events_from_resource_version(0)
            .expect_err("expected stale version error");
        assert_eq!(
            err,
            WatchEventError::StaleResourceVersion {
                requested: 0,
                oldest_available: 2
            }
        );

        let (current_version, events) = store.get_events_from_resource_version(2).unwrap();
        assert_eq!(current_version, 3);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_version, 3);
        assert_eq!(events[0].data, "v3");
    }
}
