use super::types::{EventType, WatcherEvent};

/// Event storage - circular queue
pub struct EventStore<T> {
    capacity: u32,
    cache: Vec<WatcherEvent<T>>,
    start_index: u32,
    end_index: u32,
    resource_version: u64,
}

impl<T> EventStore<T> {
    /// Get current resource version
    pub fn get_current_version(&self) -> u64 {
        self.resource_version
    }

    /// Set current resource version
    pub fn set_current_version(&mut self, version: u64) {
        self.resource_version = version;
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
        }
    }

    /// Add new event to circular queue
    pub fn mut_update(&mut self, event: WatcherEvent<T>) {
        self.resource_version = event.resource_version;

        if (self.cache.len() as u32) < self.capacity {
            self.cache.push(event);
            self.end_index = self.cache.len() as u32;
        } else {
            let index = (self.end_index % self.capacity) as usize;
            self.cache[index] = event;
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
    ) -> (Vec<WatcherEvent<T>>, u64) {
        let mut events = Vec::new();

        let count = if (self.cache.len() as u32) < self.capacity {
            self.cache.len() as u32
        } else {
            self.capacity
        };

        for i in 0..count {
            let index = ((self.start_index + i) % self.capacity) as usize;
            if index < self.cache.len() {
                let event = &self.cache[index];
                if event.resource_version > from_version {
                    events.push(event.clone());
                }
            }
        }

        (events, self.resource_version)
    }
}
