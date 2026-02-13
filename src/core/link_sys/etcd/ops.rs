//! High-level Etcd operations.
//!
//! Wraps etcd-client's interface methods with anyhow error handling and Edgion-friendly
//! signatures. Supports KV, Lease, Watch, Lock, and Health operations.
//!
//! Key operations automatically apply the namespace prefix (if configured in CRD).
//! For advanced use cases, callers can access the inner client via `get_client()`.

use std::time::Duration;

use anyhow::Result;
use etcd_client::{
    DeleteOptions, EventType, GetOptions, LockOptions as EtcdLockOptions, PutOptions, WatchOptions, WatchStream,
};

use super::client::EtcdLinkClient;
use crate::core::link_sys::redis::LinkSysHealth;

// ============================================================================
// KV Operations
// ============================================================================

impl EtcdLinkClient {
    /// GET key → Option<(value_bytes, mod_revision)>
    pub async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, i64)>> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_key = self.full_key(key);

        let resp = client
            .get(full_key, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd GET {}: {:?}", key, e))?;

        Ok(resp.kvs().first().map(|kv| (kv.value().to_vec(), kv.mod_revision())))
    }

    /// GET key → Option<String> (convenience for string values)
    pub async fn get_string(&self, key: &str) -> Result<Option<String>> {
        match self.get(key).await? {
            Some((bytes, _)) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
            None => Ok(None),
        }
    }

    /// PUT key value [with optional lease]
    pub async fn put(&self, key: &str, value: impl Into<Vec<u8>>, lease_id: Option<i64>) -> Result<()> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_key = self.full_key(key);

        let options = lease_id.map(|id| PutOptions::new().with_lease(id));
        client
            .put(full_key, value, options)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd PUT {}: {:?}", key, e))?;
        Ok(())
    }

    /// PUT key string_value (convenience)
    pub async fn put_string(&self, key: &str, value: &str) -> Result<()> {
        self.put(key, value.as_bytes().to_vec(), None).await
    }

    /// DELETE key → number of deleted keys
    pub async fn delete(&self, key: &str) -> Result<i64> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_key = self.full_key(key);

        let resp = client
            .delete(full_key, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd DELETE {}: {:?}", key, e))?;
        Ok(resp.deleted())
    }

    /// GET with prefix → Vec<(key, value_bytes, mod_revision)>
    ///
    /// Keys in the result have the namespace prefix stripped (if configured).
    pub async fn get_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>, i64)>> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_prefix = self.full_key(prefix);

        let options = GetOptions::new().with_prefix();
        let resp = client
            .get(full_prefix, Some(options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd GET prefix {}: {:?}", prefix, e))?;

        let ns_len = self.namespace().map(|n| n.len()).unwrap_or(0);
        Ok(resp
            .kvs()
            .iter()
            .map(|kv| {
                let key = kv.key_str().unwrap_or("").to_string();
                // Strip namespace prefix from returned key
                let stripped = if ns_len > 0 && key.len() > ns_len {
                    key[ns_len..].to_string()
                } else {
                    key
                };
                (stripped, kv.value().to_vec(), kv.mod_revision())
            })
            .collect())
    }

    /// DELETE with prefix → number of deleted keys
    pub async fn delete_prefix(&self, prefix: &str) -> Result<i64> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_prefix = self.full_key(prefix);

        let options = DeleteOptions::new().with_prefix();
        let resp = client
            .delete(full_prefix, Some(options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd DELETE prefix {}: {:?}", prefix, e))?;
        Ok(resp.deleted())
    }
}

// ============================================================================
// Lease Operations
// ============================================================================

impl EtcdLinkClient {
    /// Grant a lease with the given TTL (in seconds). Returns lease_id.
    pub async fn lease_grant(&self, ttl_secs: i64) -> Result<i64> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;

        let resp = client
            .lease_grant(ttl_secs, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LEASE GRANT: {:?}", e))?;
        Ok(resp.id())
    }

    /// Revoke a lease (and delete all attached keys).
    pub async fn lease_revoke(&self, lease_id: i64) -> Result<()> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;

        client
            .lease_revoke(lease_id)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LEASE REVOKE {}: {:?}", lease_id, e))?;
        Ok(())
    }

    /// Start keep-alive for a lease.
    /// Returns (LeaseKeeper, LeaseKeepAliveStream) — caller should spawn a task to drive it.
    pub async fn lease_keep_alive(
        &self,
        lease_id: i64,
    ) -> Result<(etcd_client::LeaseKeeper, etcd_client::LeaseKeepAliveStream)> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;

        let (keeper, stream) = client
            .lease_keep_alive(lease_id)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LEASE KEEP_ALIVE {}: {:?}", lease_id, e))?;
        Ok((keeper, stream))
    }

    /// Get lease TTL remaining.
    pub async fn lease_time_to_live(&self, lease_id: i64) -> Result<i64> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;

        let resp = client
            .lease_time_to_live(lease_id, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LEASE TTL {}: {:?}", lease_id, e))?;
        Ok(resp.ttl())
    }
}

// ============================================================================
// Watch Operations
// ============================================================================

/// Watch event with parsed fields.
#[derive(Debug)]
pub struct WatchEvent {
    pub key: String,
    pub value: Option<Vec<u8>>,
    pub event_type: WatchEventType,
    pub mod_revision: i64,
}

/// Watch event type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEventType {
    Put,
    Delete,
}

impl EtcdLinkClient {
    /// Watch a key for changes.
    /// Returns a WatchStream — caller should spawn a task to process events.
    pub async fn watch(&self, key: &str) -> Result<WatchStream> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_key = self.full_key(key);

        let stream = client
            .watch(full_key, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd WATCH {}: {:?}", key, e))?;
        Ok(stream)
    }

    /// Watch all keys with a given prefix.
    /// Returns a WatchStream — caller should spawn a task to process events.
    pub async fn watch_prefix(&self, prefix: &str) -> Result<WatchStream> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_prefix = self.full_key(prefix);

        let options = WatchOptions::new().with_prefix();
        let stream = client
            .watch(full_prefix, Some(options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd WATCH prefix {}: {:?}", prefix, e))?;
        Ok(stream)
    }

    /// Parse watch events from a WatchResponse, stripping namespace prefix.
    pub fn parse_watch_events(&self, resp: &etcd_client::WatchResponse) -> Vec<WatchEvent> {
        let ns_len = self.namespace().map(|n| n.len()).unwrap_or(0);
        resp.events()
            .iter()
            .filter_map(|event| {
                let kv = event.kv()?;
                let key = kv.key_str().unwrap_or("").to_string();
                let stripped = if ns_len > 0 && key.len() > ns_len {
                    key[ns_len..].to_string()
                } else {
                    key
                };
                Some(WatchEvent {
                    key: stripped,
                    value: Some(kv.value().to_vec()),
                    event_type: match event.event_type() {
                        EventType::Put => WatchEventType::Put,
                        EventType::Delete => WatchEventType::Delete,
                    },
                    mod_revision: kv.mod_revision(),
                })
            })
            .collect()
    }
}

// ============================================================================
// Distributed Lock (etcd native Lock API)
// ============================================================================

/// Etcd lock guard. Wraps the lock key returned by etcd Lock API.
/// Auto-unlocks on drop via spawned task.
pub struct EtcdLockGuard {
    client: etcd_client::Client,
    lock_key: Vec<u8>,
    lease_id: i64,
}

impl EtcdLockGuard {
    /// Explicitly release the lock.
    pub async fn unlock(mut self) -> Result<()> {
        self.client
            .unlock(self.lock_key.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Etcd UNLOCK: {:?}", e))?;
        self.client
            .lease_revoke(self.lease_id)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LEASE REVOKE (unlock): {:?}", e))?;
        Ok(())
    }

    /// Get the lock key (for debugging).
    pub fn lock_key(&self) -> &[u8] {
        &self.lock_key
    }
}

impl Drop for EtcdLockGuard {
    fn drop(&mut self) {
        // Best-effort release via spawned task.
        // If the task can't run (runtime shutting down), the lock expires via lease TTL.
        let mut client = self.client.clone();
        let lock_key = self.lock_key.clone();
        let lease_id = self.lease_id;
        tokio::spawn(async move {
            let _ = client.unlock(lock_key).await;
            let _ = client.lease_revoke(lease_id).await;
        });
    }
}

impl EtcdLinkClient {
    /// Acquire a distributed lock using etcd's native Lock API.
    ///
    /// Unlike Redis locks (SET NX + Lua), etcd locks are based on
    /// Lease + MVCC revision and provide linearizable consistency.
    /// The lock blocks until acquired (or cancelled by the lease expiring).
    pub async fn lock(&self, lock_name: &str, ttl_secs: i64) -> Result<EtcdLockGuard> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_name = self.full_key(lock_name);

        // Grant a lease for the lock
        let lease_resp = client
            .lease_grant(ttl_secs, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LOCK lease_grant: {:?}", e))?;
        let lease_id = lease_resp.id();

        // Acquire lock (blocks until acquired)
        let lock_options = EtcdLockOptions::new().with_lease(lease_id);
        let lock_resp = client
            .lock(full_name, Some(lock_options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LOCK: {:?}", e))?;

        Ok(EtcdLockGuard {
            client,
            lock_key: lock_resp.key().to_vec(),
            lease_id,
        })
    }

    /// Try to acquire lock with a timeout.
    /// Returns None if the lock could not be acquired within the timeout.
    pub async fn try_lock(&self, lock_name: &str, ttl_secs: i64, timeout: Duration) -> Result<Option<EtcdLockGuard>> {
        match tokio::time::timeout(timeout, self.lock(lock_name, ttl_secs)).await {
            Ok(Ok(guard)) => Ok(Some(guard)),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(None), // Timeout — lock not acquired
        }
    }
}

// ============================================================================
// Health Check
// ============================================================================

impl EtcdLinkClient {
    /// Active health check via Maintenance.status(). Returns latency in milliseconds.
    pub async fn ping(&self) -> Result<u64> {
        let start = std::time::Instant::now();
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;

        client
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("Etcd STATUS: {:?}", e))?;
        Ok(start.elapsed().as_millis() as u64)
    }

    /// Get detailed health status for admin API exposure.
    pub async fn health_status(&self) -> LinkSysHealth {
        match self.ping().await {
            Ok(latency_ms) => LinkSysHealth {
                name: self.name().to_string(),
                system_type: "etcd".to_string(),
                connected: true,
                latency_ms: Some(latency_ms),
                error: None,
            },
            Err(e) => LinkSysHealth {
                name: self.name().to_string(),
                system_type: "etcd".to_string(),
                connected: false,
                latency_ms: None,
                error: Some(e.to_string()),
            },
        }
    }
}
