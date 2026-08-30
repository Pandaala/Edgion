//! Access Log Store for Integration Testing
//!
//! Provides an in-memory store for complete access logs, queryable via Admin API.
//! Only active when `--integration-testing-mode` is enabled.
//!
//! Design:
//! - Uses DashMap for concurrent access from multiple worker threads
//! - Serializes new-key admission so the capacity remains a strict bound
//! - Keyed by request trace_id for precise lookup
//! - TTL-based expiration to prevent unbounded memory growth
//! - Rejects new trace IDs when TTL cleanup cannot free capacity

use dashmap::{mapref::entry::Entry, DashMap};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Default TTL for access log entries (5 minutes)
const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Default maximum capacity (10,000 entries)
const DEFAULT_MAX_CAPACITY: usize = 10_000;

/// Stored access log entry with metadata
#[derive(Clone, Debug)]
struct StoredEntry {
    /// The access log JSON string
    json: String,
    /// When this entry was stored
    stored_at: Instant,
    /// Number of logging-boundary writes observed for this trace ID.
    store_count: u64,
}

/// Access Log Store
///
/// Thread-safe in-memory store for access logs during integration testing.
pub struct AccessLogStore {
    /// Stored access logs: trace_id -> StoredEntry
    entries: DashMap<String, StoredEntry>,
    /// TTL for entries
    ttl: Duration,
    /// Maximum capacity
    max_capacity: usize,
    /// Serializes capacity checks with new-key insertion.
    insertion_lock: Mutex<()>,
    /// Total successful store calls (monotonically increasing)
    total_stored: AtomicU64,
}

/// Response format for access log queries
#[derive(Serialize)]
pub struct AccessLogResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response for listing access logs
#[derive(Serialize)]
pub struct AccessLogListResponse {
    pub success: bool,
    pub count: usize,
    pub data: Vec<AccessLogListItem>,
}

/// Individual item in list response
#[derive(Serialize)]
pub struct AccessLogListItem {
    pub trace_id: String,
    pub stored_at_ms_ago: u64,
    pub store_count: u64,
}

/// Status response for the access log store
#[derive(Serialize)]
pub struct AccessLogStoreStatus {
    pub enabled: bool,
    pub entry_count: usize,
    pub total_stored: u64,
    pub max_capacity: usize,
    pub ttl_seconds: u64,
}

impl AccessLogStore {
    /// Create a new AccessLogStore with default settings
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_TTL, DEFAULT_MAX_CAPACITY)
    }

    fn with_limits(ttl: Duration, max_capacity: usize) -> Self {
        Self {
            entries: DashMap::new(),
            ttl,
            max_capacity,
            insertion_lock: Mutex::new(()),
            total_stored: AtomicU64::new(0),
        }
    }

    /// Store an access log entry
    ///
    /// If the store is at capacity, expired entries are cleaned first.
    /// If still at capacity after cleaning, a new trace ID is rejected. Updates
    /// to an existing trace ID remain accepted because they do not grow the map.
    pub fn store(&self, trace_id: String, json: String) -> Result<(), String> {
        if let Some(mut entry) = self.entries.get_mut(&trace_id) {
            Self::update_entry(&mut entry, json);
            drop(entry);
            self.record_successful_store();
            return Ok(());
        }

        // DashMap's len() and vacant-entry insertion are not atomic across
        // shards. Serialize only the new-key path, then recheck the key because
        // another writer may have inserted it while this writer waited.
        let insertion_guard = self.insertion_lock.lock();
        if let Some(mut entry) = self.entries.get_mut(&trace_id) {
            Self::update_entry(&mut entry, json);
            drop(entry);
            drop(insertion_guard);
            self.record_successful_store();
            return Ok(());
        }

        if self.entries.len() >= self.max_capacity {
            self.cleanup_expired();
            if self.entries.len() >= self.max_capacity {
                return Err("Access log store at capacity".to_string());
            }
        }

        match self.entries.entry(trace_id) {
            Entry::Occupied(mut entry) => Self::update_entry(entry.get_mut(), json),
            Entry::Vacant(entry) => {
                entry.insert(StoredEntry {
                    json,
                    stored_at: Instant::now(),
                    store_count: 1,
                });
            }
        }

        drop(insertion_guard);
        self.record_successful_store();
        Ok(())
    }

    fn record_successful_store(&self) {
        // Periodic cleanup once per 100 successful stores: fires at the 1st,
        // 101st, 201st, ... call. The first cleanup retains the just-stored item.
        let count = self.total_stored.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(100) {
            self.cleanup_expired();
        }
    }

    fn update_entry(entry: &mut StoredEntry, json: String) {
        entry.json = json;
        entry.stored_at = Instant::now();
        entry.store_count = entry.store_count.saturating_add(1);
    }

    /// Get an access log entry by trace_id
    pub fn get(&self, trace_id: &str) -> Option<String> {
        self.entries.get(trace_id).and_then(|entry| {
            if Instant::now().saturating_duration_since(entry.stored_at) < self.ttl {
                Some(entry.json.clone())
            } else {
                None
            }
        })
    }

    /// Delete an access log entry by trace_id
    pub fn delete(&self, trace_id: &str) -> bool {
        self.entries.remove(trace_id).is_some()
    }

    /// List all stored trace_ids (non-expired)
    pub fn list(&self) -> Vec<AccessLogListItem> {
        // Read-path cleanup: without it, once writes stop, expired entries linger in the
        // DashMap until the next store() call. Reading also evicts so memory and the
        // entry_count reported by status() reflect only live entries.
        self.cleanup_expired();
        let now = Instant::now();
        self.entries
            .iter()
            .filter(|entry| now.saturating_duration_since(entry.stored_at) < self.ttl)
            .map(|entry| AccessLogListItem {
                trace_id: entry.key().clone(),
                stored_at_ms_ago: now.saturating_duration_since(entry.stored_at).as_millis() as u64,
                store_count: entry.store_count,
            })
            .collect()
    }

    /// Clear all entries
    pub fn clear(&self) {
        self.entries.clear();
    }

    /// Get store status
    pub fn status(&self) -> AccessLogStoreStatus {
        // Read-path cleanup: entry_count would otherwise include TTL-expired entries that
        // store() has not yet swept, misleading any test assertion that relies on it.
        self.cleanup_expired();
        AccessLogStoreStatus {
            enabled: true,
            entry_count: self.entries.len(),
            total_stored: self.total_stored.load(Ordering::Relaxed),
            max_capacity: self.max_capacity,
            ttl_seconds: self.ttl.as_secs(),
        }
    }

    /// Remove expired entries
    fn cleanup_expired(&self) {
        let now = Instant::now();
        self.entries
            .retain(|_, entry| now.saturating_duration_since(entry.stored_at) < self.ttl);
    }
}

impl Default for AccessLogStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Global AccessLogStore instance
static GLOBAL_ACCESS_LOG_STORE: LazyLock<AccessLogStore> = LazyLock::new(AccessLogStore::new);

/// Get the global AccessLogStore instance
pub fn get_access_log_store() -> &'static AccessLogStore {
    &GLOBAL_ACCESS_LOG_STORE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn test_store_and_get() {
        let store = AccessLogStore::new();
        store
            .store("trace-001".to_string(), r#"{"test": true}"#.to_string())
            .unwrap();

        let result = store.get("trace-001");
        assert_eq!(result, Some(r#"{"test": true}"#.to_string()));
    }

    #[test]
    fn test_get_nonexistent() {
        let store = AccessLogStore::new();
        assert_eq!(store.get("nonexistent"), None);
    }

    #[test]
    fn test_delete() {
        let store = AccessLogStore::new();
        store
            .store("trace-001".to_string(), r#"{"test": true}"#.to_string())
            .unwrap();

        assert!(store.delete("trace-001"));
        assert_eq!(store.get("trace-001"), None);
    }

    #[test]
    fn test_list() {
        let store = AccessLogStore::new();
        store.store("trace-001".to_string(), "{}".to_string()).unwrap();
        store.store("trace-002".to_string(), "{}".to_string()).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|entry| entry.store_count == 1));
    }

    #[test]
    fn repeated_trace_counts_logging_boundary_writes() {
        let store = AccessLogStore::new();
        store
            .store("trace-001".to_string(), r#"{"value":1}"#.to_string())
            .unwrap();
        store
            .store("trace-001".to_string(), r#"{"value":2}"#.to_string())
            .unwrap();

        assert_eq!(store.get("trace-001"), Some(r#"{"value":2}"#.to_string()));
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].store_count, 2);
    }

    #[test]
    fn repeated_trace_crosses_cleanup_cadence_without_self_deadlock() {
        const WRITES: usize = 101;
        let store = AccessLogStore::new();
        for write in 0..WRITES {
            store
                .store("trace-001".to_string(), format!(r#"{{"write":{write}}}"#))
                .unwrap();
        }

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].store_count, WRITES as u64);
        assert_eq!(store.status().total_stored, WRITES as u64);
    }

    #[test]
    fn concurrent_first_writes_are_all_counted() {
        const WRITERS: usize = 32;
        let store = AccessLogStore::new();
        let barrier = Arc::new(Barrier::new(WRITERS));

        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let barrier = Arc::clone(&barrier);
                let store = &store;
                scope.spawn(move || {
                    barrier.wait();
                    store
                        .store("shared-trace".to_string(), format!(r#"{{"writer":{writer}}}"#))
                        .unwrap();
                });
            }
        });

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].store_count, WRITERS as u64);
    }

    #[test]
    fn concurrent_distinct_writes_cannot_exceed_capacity() {
        const WRITERS: usize = 32;
        let store = AccessLogStore::with_limits(DEFAULT_TTL, 1);
        let barrier = Arc::new(Barrier::new(WRITERS));

        let accepted = std::thread::scope(|scope| {
            let handles = (0..WRITERS)
                .map(|writer| {
                    let barrier = Arc::clone(&barrier);
                    let store = &store;
                    scope.spawn(move || {
                        barrier.wait();
                        store.store(format!("trace-{writer}"), "{}".to_string()).is_ok()
                    })
                })
                .collect::<Vec<_>>();

            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|accepted| *accepted)
                .count()
        });

        assert_eq!(accepted, 1);
        let status = store.status();
        assert_eq!(status.entry_count, 1);
        assert_eq!(status.total_stored, 1);
    }

    #[test]
    fn rejected_write_does_not_increment_total_stored() {
        let store = AccessLogStore::with_limits(DEFAULT_TTL, 1);
        store.store("trace-1".to_string(), "{}".to_string()).unwrap();
        assert!(store.store("trace-2".to_string(), "{}".to_string()).is_err());

        assert_eq!(store.status().total_stored, 1);
    }

    #[test]
    fn read_paths_remove_expired_entries() {
        let ttl = Duration::from_secs(1);
        let store = AccessLogStore::with_limits(ttl, 2);

        store.store("trace-1".to_string(), "{}".to_string()).unwrap();
        store.entries.get_mut("trace-1").unwrap().stored_at = Instant::now().checked_sub(ttl).unwrap();
        assert!(store.list().is_empty());

        store.store("trace-2".to_string(), "{}".to_string()).unwrap();
        store.entries.get_mut("trace-2").unwrap().stored_at = Instant::now().checked_sub(ttl).unwrap();
        assert_eq!(store.status().entry_count, 0);
    }

    #[test]
    fn test_clear() {
        let store = AccessLogStore::new();
        store.store("trace-001".to_string(), "{}".to_string()).unwrap();
        store.store("trace-002".to_string(), "{}".to_string()).unwrap();

        store.clear();
        assert_eq!(store.list().len(), 0);
    }

    #[test]
    fn test_status() {
        let store = AccessLogStore::new();
        store.store("trace-001".to_string(), "{}".to_string()).unwrap();

        let status = store.status();
        assert!(status.enabled);
        assert_eq!(status.entry_count, 1);
        assert_eq!(status.total_stored, 1);
        assert_eq!(status.max_capacity, DEFAULT_MAX_CAPACITY);
    }
}
