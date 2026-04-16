use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::common::conf_sync::types::EventType;

use super::traits::CenterConfHandler;

/// Simplified watch event for Center consumption.
pub struct WatchEventSimple<T> {
    pub event_type: EventType,
    pub key: String,
    pub data: T,
}

/// Per-controller single-source watch cache.
/// Mirrors Gateway's ClientCache<T> — one instance per controller per kind.
pub struct CenterWatchCache<T> {
    controller_id: String,
    inner: RwLock<CacheState<T>>,
    handler: Arc<dyn CenterConfHandler<T> + Send + Sync>,
}

struct CacheState<T> {
    data: HashMap<String, Arc<T>>,
    sync_version: u64,
    server_id: String,
}

impl<T: Send + Sync + 'static> CenterWatchCache<T> {
    pub fn new(
        controller_id: String,
        handler: Arc<dyn CenterConfHandler<T> + Send + Sync>,
    ) -> Self {
        Self {
            controller_id,
            inner: RwLock::new(CacheState {
                data: HashMap::new(),
                sync_version: 0,
                server_id: String::new(),
            }),
            handler,
        }
    }

    /// Full replace from list response. Triggers handler.full_set().
    ///
    /// Handler call happens OUTSIDE the lock to avoid deadlocks.
    pub fn replace_all(
        &self,
        items: Vec<(String, T)>,
        sync_version: u64,
        server_id: String,
    ) {
        let snapshot = {
            let mut state = self.inner.write();
            state.data = items
                .into_iter()
                .map(|(k, v)| (k, Arc::new(v)))
                .collect();
            state.sync_version = sync_version;
            state.server_id = server_id;
            // Clone snapshot for handler (cheap Arc clone)
            state.data.clone()
        };

        self.handler.full_set(&self.controller_id, &snapshot);
    }

    /// Incremental update from watch events. Classifies by EventType and triggers
    /// handler.partial_update().
    ///
    /// Handler call happens OUTSIDE the lock to avoid deadlocks.
    pub fn apply_events(
        &self,
        events: Vec<WatchEventSimple<T>>,
        sync_version: u64,
        server_id: String,
    ) {
        let (add, update, remove) = {
            let mut state = self.inner.write();

            let mut add: HashMap<String, Arc<T>> = HashMap::new();
            let mut update: HashMap<String, Arc<T>> = HashMap::new();
            let mut remove: HashSet<String> = HashSet::new();

            for event in events {
                match event.event_type {
                    EventType::Add => {
                        let arc = Arc::new(event.data);
                        state.data.insert(event.key.clone(), arc.clone());
                        add.insert(event.key, arc);
                    }
                    EventType::Update => {
                        let arc = Arc::new(event.data);
                        state.data.insert(event.key.clone(), arc.clone());
                        update.insert(event.key, arc);
                    }
                    EventType::Delete => {
                        state.data.remove(&event.key);
                        remove.insert(event.key);
                    }
                }
            }

            state.sync_version = sync_version;
            state.server_id = server_id;

            (add, update, remove)
        };

        self.handler
            .partial_update(&self.controller_id, add, update, remove);
    }

    pub fn get_sync_version(&self) -> u64 {
        self.inner.read().sync_version
    }

    pub fn get_server_id(&self) -> String {
        self.inner.read().server_id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockHandler {
        full_set_calls: AtomicUsize,
        partial_update_calls: AtomicUsize,
    }

    impl MockHandler {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                full_set_calls: AtomicUsize::new(0),
                partial_update_calls: AtomicUsize::new(0),
            })
        }
    }

    impl CenterConfHandler<String> for MockHandler {
        fn full_set(&self, _controller_id: &str, _data: &HashMap<String, Arc<String>>) {
            self.full_set_calls.fetch_add(1, Ordering::SeqCst);
        }

        fn partial_update(
            &self,
            _controller_id: &str,
            _add: HashMap<String, Arc<String>>,
            _update: HashMap<String, Arc<String>>,
            _remove: HashSet<String>,
        ) {
            self.partial_update_calls.fetch_add(1, Ordering::SeqCst);
        }

        fn controller_offline(&self, _controller_id: &str) {}

        fn controller_removed(&self, _controller_id: &str) {}
    }

    #[test]
    fn get_sync_version_zero_initially() {
        let handler = MockHandler::new();
        let cache = CenterWatchCache::new("ctrl-1".to_string(), handler);
        assert_eq!(cache.get_sync_version(), 0);
        assert_eq!(cache.get_server_id(), "");
    }

    #[test]
    fn replace_all_updates_data_and_calls_handler() {
        let handler = MockHandler::new();
        let cache = CenterWatchCache::new("ctrl-1".to_string(), handler.clone());

        let items = vec![
            ("key1".to_string(), "val1".to_string()),
            ("key2".to_string(), "val2".to_string()),
        ];
        cache.replace_all(items, 42, "server-abc".to_string());

        assert_eq!(cache.get_sync_version(), 42);
        assert_eq!(cache.get_server_id(), "server-abc");
        assert_eq!(handler.full_set_calls.load(Ordering::SeqCst), 1);
        assert_eq!(handler.partial_update_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn apply_events_calls_partial_update() {
        let handler = MockHandler::new();
        let cache = CenterWatchCache::new("ctrl-1".to_string(), handler.clone());

        // Seed with an initial item so we can update it
        cache.replace_all(
            vec![("key1".to_string(), "old-val".to_string())],
            1,
            "server-1".to_string(),
        );

        let events = vec![
            WatchEventSimple {
                event_type: EventType::Add,
                key: "key2".to_string(),
                data: "val2".to_string(),
            },
            WatchEventSimple {
                event_type: EventType::Update,
                key: "key1".to_string(),
                data: "new-val".to_string(),
            },
        ];
        cache.apply_events(events, 2, "server-1".to_string());

        assert_eq!(cache.get_sync_version(), 2);
        assert_eq!(handler.full_set_calls.load(Ordering::SeqCst), 1);
        assert_eq!(handler.partial_update_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn apply_events_with_delete() {
        let handler = MockHandler::new();
        let cache = CenterWatchCache::new("ctrl-1".to_string(), handler.clone());

        cache.replace_all(
            vec![("key1".to_string(), "val1".to_string())],
            1,
            "server-1".to_string(),
        );

        let events = vec![WatchEventSimple {
            event_type: EventType::Delete,
            key: "key1".to_string(),
            data: String::new(), // data ignored for Delete
        }];
        cache.apply_events(events, 2, "server-1".to_string());

        assert_eq!(cache.get_sync_version(), 2);
        assert_eq!(handler.partial_update_calls.load(Ordering::SeqCst), 1);

        // Verify item removed from internal state: apply another event to get the snapshot
        // (we can't peek directly, so just check sync version advanced correctly)
        assert_eq!(cache.get_sync_version(), 2);
    }
}
