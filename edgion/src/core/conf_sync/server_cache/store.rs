use super::types::{EventType, WatcherEvent};
use std::collections::HashMap;

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
            resource_version: 0,
            expire_version: 0,
            data: HashMap::new(),
        }
    }

    /// Add new event to circular queue
    pub fn mut_update(&mut self, event: WatcherEvent<T>) {
        let index = self.end_index % self.capacity;

        if self.end_index - self.start_index >= self.capacity {
            if let Some(last_event) = self.cache[index].as_ref() {
                self.expire_version = last_event.resource_version;
            }
            self.start_index += 1;
        }

        self.cache[index] = Some(event);
        self.end_index += 1;
    }

    /// Get events starting from specified version
    pub fn get_events_from_resource_version(
        &self,
        from_version: u64,
    ) -> Result<(u64, Option<Vec<WatcherEvent<T>>>), String> {
        if from_version > self.resource_version {
            return Err("VersionUnexpect".to_owned());
        } else if from_version == self.resource_version {
            return Ok((self.resource_version, None));
        }

        if from_version != 0 && from_version < self.expire_version {
            return Err("TooOldVersion".to_owned());
        }

        if self.capacity == 0 || self.end_index == self.start_index {
            return Ok((self.resource_version, None));
        }

        // Walk backward to find the earliest index whose version is > from_version.
        let mut start_scan = self.end_index;
        while start_scan > self.start_index {
            let idx = (start_scan - 1) % self.capacity;
            match self.cache[idx].as_ref() {
                Some(ev) if ev.resource_version > from_version => {
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
                if ev.resource_version > from_version {
                    events.push(ev.clone());
                }
            }
            loop_index += 1;
        }

        Ok((self.resource_version, Some(events)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> EventStore<String> {
        EventStore::new(10)
    }

    #[test]
    fn empty_store_returns_no_events() {
        let store = make_store();

        let (current_version, events) = store.get_events_from_resource_version(0).unwrap();

        assert_eq!(current_version, 0);
        assert!(events.is_none());
    }

    #[test]
    fn apply_event_adds_data_and_updates_version() {
        let mut store = make_store();

        store.apply_event(EventType::Add, "alpha".to_string(), 1);

        let (snapshot, version) = store.snapshot_owned();
        assert_eq!(version, 1);
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot.contains(&"alpha".to_string()));

        let (current_version, events_opt) = store.get_events_from_resource_version(0).unwrap();
        let events = events_opt.expect("expected events");
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

        let (_, events_opt) = store.get_events_from_resource_version(1).unwrap();
        let events = events_opt.expect("expected events for version > 1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_version, 2);
        assert_eq!(events[0].data, "beta");
        assert!(matches!(events[0].event_type, EventType::Update));

        let (_, events_opt) = store.get_events_from_resource_version(2).unwrap();
        assert!(
            events_opt.is_none(),
            "no events expected when requesting current version"
        );
    }

    #[test]
    fn stale_version_error_when_events_expired() {
        let mut store: EventStore<String> = EventStore::new(50);

        for version in 1..=60 {
            store.apply_event(EventType::Add, format!("v{version}"), version);
        }

        let err = store
            .get_events_from_resource_version(5)
            .expect_err("expected stale version error");
        assert_eq!(err, "TooOldVersion");

        let (current_version, events_opt) = store.get_events_from_resource_version(55).unwrap();
        let events = events_opt.expect("expected events after catching up");
        assert_eq!(current_version, 60);
        let versions: Vec<u64> = events.iter().map(|ev| ev.resource_version).collect();
        assert_eq!(versions, vec![56, 57, 58, 59, 60]);
    }

    #[test]
    fn multiple_wraps_over_capacity() {
        let mut store: EventStore<String> = EventStore::new(50);

        for version in 1..=120 {
            store.apply_event(EventType::Add, format!("value-{version}"), version);
        }

        let (current_version, events_opt) = store.get_events_from_resource_version(110).unwrap();
        let events = events_opt.expect("expected events after wrap");
        assert_eq!(current_version, 120);
        let versions: Vec<u64> = events.iter().map(|ev| ev.resource_version).collect();
        assert_eq!(versions, (111..=120).collect::<Vec<_>>());

        for (offset, event) in events.iter().enumerate() {
            assert_eq!(event.data, format!("value-{}", 111 + offset as u64));
        }

        let err = store
            .get_events_from_resource_version(10)
            .expect_err("versions older than expire_version should error");
        assert_eq!(err, "TooOldVersion");
    }

    #[test]
    fn version_unexpect_error_when_requesting_future_version() {
        let mut store = make_store();

        store.apply_event(EventType::Add, "alpha".to_string(), 1);

        let err = store
            .get_events_from_resource_version(99)
            .expect_err("requesting future version should error");
        assert_eq!(err, "VersionUnexpect");
    }
}
