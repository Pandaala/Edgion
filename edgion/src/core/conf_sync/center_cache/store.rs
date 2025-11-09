use super::types::{EventType, WatcherEvent};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

/// Event storage - circular queue
pub struct EventStore<T> {
    capacity: u32,
    cache: Vec<(u64, WatcherEvent<T>)>,
    start_index: u32,
    end_index: u32,
    resource_version: u64,
    sequence: u64,
    data: HashMap<String, T>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEventError {
    StaleResourceVersion {
        requested: u64,
        oldest_available: u64,
    },
}

impl fmt::Display for WatchEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchEventError::StaleResourceVersion {
                requested,
                oldest_available,
            } => write!(
                f,
                "requested version {} is older than oldest available {}",
                requested, oldest_available
            ),
        }
    }
}

impl Error for WatchEventError {}

impl<T> EventStore<T> {
    /// Set current resource version
    pub fn set_current_version(&mut self, version: u64) {
        self.sequence = version;
        self.resource_version = version;
    }

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
        (data, self.sequence)
    }
}

impl<T: Clone> EventStore<T> {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            cache: Vec::with_capacity(capacity as usize),
            start_index: 0,
            end_index: 0,
            resource_version: 0,
            sequence: 0,
            data: HashMap::new(),
        }
    }

    /// Add new event to circular queue
    pub fn mut_update(&mut self, event: WatcherEvent<T>) {
        self.sequence = self.sequence.wrapping_add(1);
        self.resource_version = event.resource_version;
        let seq = self.sequence;

        if (self.cache.len() as u32) < self.capacity {
            self.cache.push((seq, event));
            self.end_index = self.cache.len() as u32;
        } else {
            let index = (self.end_index % self.capacity) as usize;
            self.cache[index] = (seq, event);
            self.end_index = self.end_index.wrapping_add(1);

            if self.end_index.wrapping_sub(self.start_index) > self.capacity {
                self.start_index = self.end_index.wrapping_sub(self.capacity);
            }
        }
    }

    /// Get events starting from specified version
    pub fn get_events_from_resource_version(
        &self,
        from_version: u64,
    ) -> Result<(u64, Vec<WatcherEvent<T>>), WatchEventError> {
        if let Some(oldest_version) = self.oldest_version() {
            if from_version != 0 && from_version < oldest_version {
                return Err(WatchEventError::StaleResourceVersion {
                    requested: from_version,
                    oldest_available: oldest_version,
                });
            }
        }

        let mut events = Vec::new();

        let count = if (self.cache.len() as u32) < self.capacity {
            self.cache.len() as u32
        } else {
            self.capacity
        };

        for i in 0..count {
            let index = ((self.start_index + i) % self.capacity) as usize;
            if index < self.cache.len() {
                let (seq, event) = &self.cache[index];
                if *seq > from_version {
                    events.push(event.clone());
                }
            }
        }

        Ok((self.sequence, events))
    }

    fn oldest_version(&self) -> Option<u64> {
        if self.cache.is_empty() {
            None
        } else if (self.cache.len() as u32) < self.capacity {
            self.cache.first().map(|(seq, _)| *seq)
        } else {
            let index = (self.start_index % self.capacity) as usize;
            self.cache.get(index).map(|(seq, _)| *seq)
        }
    }
}
