use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::cache::CenterWatchCache;
use super::traits::CenterConfHandler;

/// Manages CenterWatchCache instances for a single resource kind.
/// Caches persist across controller reconnects to preserve sync_version.
pub struct CenterWatchCacheRegistry<T> {
    caches: RwLock<HashMap<String, Arc<CenterWatchCache<T>>>>,
    handler: Arc<dyn CenterConfHandler<T> + Send + Sync>,
}

impl<T: Send + Sync + 'static> CenterWatchCacheRegistry<T> {
    pub fn new(handler: Arc<dyn CenterConfHandler<T> + Send + Sync>) -> Self {
        Self {
            caches: RwLock::new(HashMap::new()),
            handler,
        }
    }

    /// Get or create a cache for a controller.
    /// On reconnect, returns the existing cache so sync_version is preserved.
    pub fn get_or_create(&self, controller_id: &str) -> Arc<CenterWatchCache<T>> {
        // Fast path: read lock
        {
            let caches = self.caches.read();
            if let Some(cache) = caches.get(controller_id) {
                return cache.clone();
            }
        }

        // Slow path: write lock, double-check
        let mut caches = self.caches.write();
        caches
            .entry(controller_id.to_string())
            .or_insert_with(|| Arc::new(CenterWatchCache::new(controller_id.to_string(), self.handler.clone())))
            .clone()
    }

    /// List all controllers with their sync state.
    /// Returns (controller_id, sync_version, server_id).
    pub fn list_controllers(&self) -> Vec<(String, u64, String)> {
        self.caches
            .read()
            .iter()
            .map(|(id, cache)| (id.clone(), cache.get_sync_version(), cache.get_server_id()))
            .collect()
    }

    /// Mark controller offline. Preserves cache for reconnect.
    pub fn mark_offline(&self, controller_id: &str) {
        self.handler.controller_offline(controller_id);
    }

    /// Remove controller entirely. Deletes cache and notifies handler.
    pub fn remove_controller(&self, controller_id: &str) {
        self.caches.write().remove(controller_id);
        self.handler.controller_removed(controller_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockHandler {
        offline_count: AtomicUsize,
        removed_count: AtomicUsize,
    }

    impl MockHandler {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                offline_count: AtomicUsize::new(0),
                removed_count: AtomicUsize::new(0),
            })
        }
    }

    impl CenterConfHandler<String> for MockHandler {
        fn full_set(&self, _controller_id: &str, _data: &HashMap<String, Arc<String>>) {}

        fn partial_update(
            &self,
            _controller_id: &str,
            _add: HashMap<String, Arc<String>>,
            _update: HashMap<String, Arc<String>>,
            _remove: HashSet<String>,
        ) {
        }

        fn controller_offline(&self, _controller_id: &str) {
            self.offline_count.fetch_add(1, Ordering::SeqCst);
        }

        fn controller_removed(&self, _controller_id: &str) {
            self.removed_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn get_or_create_returns_same_instance() {
        let handler = MockHandler::new();
        let registry = CenterWatchCacheRegistry::<String>::new(handler);

        let cache1 = registry.get_or_create("ctrl-1");
        let cache2 = registry.get_or_create("ctrl-1");

        assert!(Arc::ptr_eq(&cache1, &cache2));
    }

    #[test]
    fn get_or_create_different_controllers() {
        let handler = MockHandler::new();
        let registry = CenterWatchCacheRegistry::<String>::new(handler);

        let cache1 = registry.get_or_create("ctrl-1");
        let cache2 = registry.get_or_create("ctrl-2");

        assert!(!Arc::ptr_eq(&cache1, &cache2));
    }

    #[test]
    fn mark_offline_calls_handler_preserves_cache() {
        let handler = MockHandler::new();
        let registry = CenterWatchCacheRegistry::<String>::new(handler.clone());

        let cache = registry.get_or_create("ctrl-1");
        // Simulate some sync progress
        cache.replace_all(
            vec![("key1".to_string(), "val1".to_string())],
            99,
            "server-1".to_string(),
        );

        registry.mark_offline("ctrl-1");

        assert_eq!(handler.offline_count.load(Ordering::SeqCst), 1);
        assert_eq!(handler.removed_count.load(Ordering::SeqCst), 0);

        // Cache is preserved: same instance returned, sync_version still 99
        let cache_after = registry.get_or_create("ctrl-1");
        assert!(Arc::ptr_eq(&cache, &cache_after));
        assert_eq!(cache_after.get_sync_version(), 99);
    }

    #[test]
    fn remove_controller_clears_cache() {
        let handler = MockHandler::new();
        let registry = CenterWatchCacheRegistry::<String>::new(handler.clone());

        let cache = registry.get_or_create("ctrl-1");
        cache.replace_all(
            vec![("key1".to_string(), "val1".to_string())],
            42,
            "server-1".to_string(),
        );

        registry.remove_controller("ctrl-1");

        assert_eq!(handler.removed_count.load(Ordering::SeqCst), 1);

        // New get_or_create must return a fresh cache with sync_version = 0
        let fresh_cache = registry.get_or_create("ctrl-1");
        assert!(!Arc::ptr_eq(&cache, &fresh_cache));
        assert_eq!(fresh_cache.get_sync_version(), 0);
    }
}
