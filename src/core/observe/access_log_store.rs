//! Access Log Store for Integration Testing
//!
//! Provides an in-memory store for complete access logs, queryable via Admin API.
//! Only active when `--integration-testing-mode` is enabled.
//!
//! Design:
//! - Uses DashMap for lock-free concurrent access from multiple worker threads
//! - Keyed by request trace_id for precise lookup
//! - TTL-based expiration to prevent unbounded memory growth
//! - Capacity limit with oldest-entry eviction

use dashmap::DashMap;
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
    /// Total entries ever stored (monotonically increasing)
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
        Self {
            entries: DashMap::new(),
            ttl: DEFAULT_TTL,
            max_capacity: DEFAULT_MAX_CAPACITY,
            total_stored: AtomicU64::new(0),
        }
    }

    /// Store an access log entry
    ///
    /// If the store is at capacity, expired entries are cleaned first.
    /// If still at capacity after cleaning, the store accepts the new entry
    /// (DashMap doesn't support ordered eviction, so we rely on TTL cleanup).
    pub fn store(&self, trace_id: String, json: String) -> Result<(), String> {
        // Periodic cleanup: on every 100th store, clean expired entries
        let count = self.total_stored.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(100) {
            self.cleanup_expired();
        }

        // Check capacity
        if self.entries.len() >= self.max_capacity {
            self.cleanup_expired();
            if self.entries.len() >= self.max_capacity {
                return Err("Access log store at capacity".to_string());
            }
        }

        self.entries.insert(
            trace_id,
            StoredEntry {
                json,
                stored_at: Instant::now(),
            },
        );

        Ok(())
    }

    /// Get an access log entry by trace_id
    pub fn get(&self, trace_id: &str) -> Option<String> {
        self.entries.get(trace_id).and_then(|entry| {
            if entry.stored_at.elapsed() < self.ttl {
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
        let now = Instant::now();
        self.entries
            .iter()
            .filter(|entry| entry.stored_at.elapsed() < self.ttl)
            .map(|entry| AccessLogListItem {
                trace_id: entry.key().clone(),
                stored_at_ms_ago: now.duration_since(entry.stored_at).as_millis() as u64,
            })
            .collect()
    }

    /// Clear all entries
    pub fn clear(&self) {
        self.entries.clear();
    }

    /// Get store status
    pub fn status(&self) -> AccessLogStoreStatus {
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
        self.entries.retain(|_, entry| entry.stored_at.elapsed() < self.ttl);
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
