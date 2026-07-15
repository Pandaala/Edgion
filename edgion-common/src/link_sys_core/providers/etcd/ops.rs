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
    Compare, CompareOp, DeleteOptions, GetOptions, LockOptions as EtcdLockOptions, PutOptions, Txn, TxnOp,
    WatchOptions, WatchStream,
};

use super::client::EtcdLinkClient;
use super::config_mapping::health_probe_timeout;
use crate::link_sys_core::LinkSysHealth;

/// Raw view of an etcd KeyValue exposing the fields leader election needs.
///
/// The existing `get()` intentionally returns only `(value, mod_revision)`;
/// election's guard txn (spec §4.4) additionally needs `lease` and
/// `create_revision`.
#[derive(Debug, Clone)]
pub struct KeyValueRaw {
    pub value: Vec<u8>,
    pub lease: i64,
    pub mod_revision: i64,
    pub create_revision: i64,
}

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

    /// GET with prefix → `(items, cluster_revision)`.
    ///
    /// Same as `get_prefix` but also returns the cluster-level revision from the
    /// response header. This is needed for ConfCenter init-then-watch: after the
    /// initial list, the watcher starts from `cluster_revision + 1`.
    pub async fn get_prefix_with_revision(&self, prefix: &str) -> Result<(Vec<(String, Vec<u8>, i64)>, i64)> {
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

        let cluster_revision = resp.header().map(|h| h.revision()).unwrap_or(0);

        let ns_len = self.namespace().map(|n| n.len()).unwrap_or(0);
        let items = resp
            .kvs()
            .iter()
            .map(|kv| {
                let key = kv.key_str().unwrap_or("").to_string();
                let stripped = if ns_len > 0 && key.len() > ns_len {
                    key[ns_len..].to_string()
                } else {
                    key
                };
                (stripped, kv.value().to_vec(), kv.mod_revision())
            })
            .collect();

        Ok((items, cluster_revision))
    }

    /// PUT key value only if the key does not yet exist (version == 0).
    /// Returns `true` if the key was created, `false` if it already existed.
    pub async fn txn_put_if_absent(&self, key: &str, value: impl Into<Vec<u8>>) -> Result<bool> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_key = self.full_key(key);
        let value = value.into();

        let txn = Txn::new()
            .when(vec![Compare::version(full_key.clone(), CompareOp::Equal, 0)])
            .and_then(vec![TxnOp::put(full_key, value, None)]);

        let resp = client
            .txn(txn)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd TXN put_if_absent {}: {:?}", key, e))?;
        Ok(resp.succeeded())
    }

    /// PUT key value only if the key already exists (version > 0).
    /// Returns `true` if the key was updated, `false` if it did not exist.
    pub async fn txn_put_if_exists(&self, key: &str, value: impl Into<Vec<u8>>) -> Result<bool> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_key = self.full_key(key);
        let value = value.into();

        let txn = Txn::new()
            .when(vec![Compare::version(full_key.clone(), CompareOp::Greater, 0)])
            .and_then(vec![TxnOp::put(full_key, value, None)]);

        let resp = client
            .txn(txn)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd TXN put_if_exists {}: {:?}", key, e))?;
        Ok(resp.succeeded())
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

    /// Linearizable single-key read exposing lease + both revisions.
    ///
    /// This is the raw read leader election needs (spec §4.3.1/§4.4): the
    /// existing `get()` discards `lease`/`create_revision`.
    pub async fn get_kv(&self, key: &str) -> Result<Option<KeyValueRaw>> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full = self.full_key(key);
        let resp = client
            .get(full, None)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd GET_KV {}: {:?}", key, e))?;
        Ok(resp.kvs().first().map(|kv| KeyValueRaw {
            value: kv.value().to_vec(),
            lease: kv.lease(),
            mod_revision: kv.mod_revision(),
            create_revision: kv.create_revision(),
        }))
    }

    /// Guard txn (spec §4.4). Returns `Ok(true)` iff the key is STILL ours by all
    /// three compares. All three are required: `create_revision` alone survives an
    /// in-place overwrite; `lease` catches a lease detach; `value` catches any
    /// overwrite. `Ok(false)` is authoritative demotion; `Err` is transport.
    pub async fn leader_guard_txn(
        &self,
        key: &str,
        create_revision: i64,
        lease_id: i64,
        expected_value: &[u8],
    ) -> Result<bool> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full = self.full_key(key);
        let txn = Txn::new().when(vec![
            Compare::create_revision(full.clone(), CompareOp::Equal, create_revision),
            Compare::lease(full.clone(), CompareOp::Equal, lease_id),
            Compare::value(full, CompareOp::Equal, expected_value.to_vec()),
        ]);
        let resp = client
            .txn(txn)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd GUARD_TXN {}: {:?}", key, e))?;
        Ok(resp.succeeded())
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

    /// Establish a keepalive stream for `lease_id`.
    ///
    /// IMPORTANT (spec §4.2): etcd-client reads the first response inside this
    /// establish call; if the lease is already gone (`ttl <= 0`) it returns
    /// `Error::LeaseKeepAliveError`. Callers MUST treat this `Err` as
    /// authoritative lease death, not as a transient transport error. Driving
    /// the returned keeper only pushes into a bounded channel and is NOT
    /// evidence the lease is alive; only a `LeaseKeepAliveResponse` with
    /// `ttl > 0` off the stream is.
    pub async fn lease_keep_alive(
        &self,
        lease_id: i64,
    ) -> Result<(etcd_client::LeaseKeeper, etcd_client::LeaseKeepAliveStream)> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        client
            .lease_keep_alive(lease_id)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd LEASE KEEPALIVE {}: {:?}", lease_id, e))
    }
}

// ============================================================================
// Watch Operations
// ============================================================================

impl EtcdLinkClient {
    /// Watch all keys with a given prefix, starting from a specific revision.
    ///
    /// Used by ConfCenter to resume watching after init-phase or reconnection.
    /// Events with `mod_revision <= start_revision` are excluded by etcd.
    pub async fn watch_prefix_from_revision(&self, prefix: &str, start_revision: i64) -> Result<WatchStream> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full_prefix = self.full_key(prefix);

        let options = WatchOptions::new().with_prefix().with_start_revision(start_revision);
        let stream = client
            .watch(full_prefix, Some(options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd WATCH prefix (rev={}) {}: {:?}", start_revision, prefix, e))?;
        Ok(stream)
    }

    /// Watch a single key from a revision (single-key analogue of
    /// `watch_prefix_from_revision`). Used for a leader's self-watch / a
    /// follower's leader-key watch as a latency optimization on top of polling.
    pub async fn watch_key_from_revision(&self, key: &str, start_revision: i64) -> Result<WatchStream> {
        let mut client = self
            .get_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("Etcd [{}] not connected", self.name()))?;
        let full = self.full_key(key);
        let options = WatchOptions::new().with_start_revision(start_revision);
        let stream = client
            .watch(full, Some(options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd WATCH key (rev={}) {}: {:?}", start_revision, key, e))?;
        Ok(stream)
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
    ///
    /// Crate-private: callers must go through `try_lock` so the wait is bounded.
    async fn lock(&self, lock_name: &str, ttl_secs: i64) -> Result<EtcdLockGuard> {
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

        let d = health_probe_timeout(self.config());
        match tokio::time::timeout(d, client.status()).await {
            Ok(Ok(_)) => Ok(start.elapsed().as_millis() as u64),
            Ok(Err(e)) => Err(anyhow::anyhow!("Etcd STATUS: {:?}", e)),
            Err(_) => Err(anyhow::anyhow!("Etcd STATUS timeout after {:?}", d)),
        }
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

// ============================================================================
// Tests
// ============================================================================
//
// Every test here talks to a REAL etcd; a mock gives near-zero confidence in
// the lease/txn failure modes these primitives target (spec §10). They are
// therefore `#[ignore]`-gated so the offline `cargo test` stays green, and are
// run under `edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys_core::credential::CredentialState;
    use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

    /// Minimal config pointing at a local etcd, mirroring the fixture in
    /// `client.rs`'s test module.
    fn minimal_config() -> EtcdClientConfig {
        EtcdClientConfig {
            endpoints: vec!["http://localhost:2379".to_string()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
            auto_sync_interval: None,
            max_call_send_size: None,
            max_call_recv_size: None,
            user_agent: None,
            reject_old_cluster: None,
            observability: None,
        }
    }

    /// Build + connect an `EtcdLinkClient` against a local etcd.
    async fn test_client() -> EtcdLinkClient {
        let config = minimal_config();
        let client = EtcdLinkClient::from_config("default/etcd-test", &config, &CredentialState::NotRequired)
            .expect("from_config must succeed");
        client.init().await.expect("etcd init must connect");
        client
    }

    /// A per-test unique key prefix so parallel/repeated runs never collide.
    fn unique_prefix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("/edgion-test/{}", nanos)
    }

    #[tokio::test]
    #[ignore = "requires a real etcd on localhost:2379"]
    async fn lease_keep_alive_refreshes_then_errors_after_revoke() {
        let client = test_client().await;
        let lease = client.lease_grant(5).await.unwrap();
        let (mut keeper, mut stream) = client.lease_keep_alive(lease).await.unwrap();
        keeper.keep_alive().await.unwrap();
        let resp = stream.message().await.unwrap().expect("a keepalive response");
        assert!(resp.ttl() > 0, "healthy lease must report ttl > 0");

        client.lease_revoke(lease).await.unwrap();
        // A re-establish against the now-dead lease must surface as Err (spec §4.2 fact 4).
        let err = client.lease_keep_alive(lease).await;
        assert!(err.is_err(), "keepalive establish on a revoked lease must Err");
    }

    #[tokio::test]
    #[ignore = "requires a real etcd on localhost:2379"]
    async fn get_kv_surfaces_lease_and_revisions() {
        let client = test_client().await;
        let lease = client.lease_grant(30).await.unwrap();
        let key = format!("{}/kvtest", unique_prefix());
        client.put(&key, "v", Some(lease)).await.unwrap();
        let kv = client.get_kv(&key).await.unwrap().expect("present");
        assert_eq!(kv.value, b"v");
        assert_eq!(kv.lease, lease);
        assert!(kv.create_revision > 0 && kv.mod_revision >= kv.create_revision);
    }

    #[tokio::test]
    #[ignore = "requires a real etcd on localhost:2379"]
    async fn guard_txn_detects_inplace_overwrite() {
        let client = test_client().await;
        let lease = client.lease_grant(30).await.unwrap();
        let key = format!("{}/guardtest", unique_prefix());
        client.put(&key, "mine", Some(lease)).await.unwrap();
        let kv = client.get_kv(&key).await.unwrap().unwrap();
        assert!(client
            .leader_guard_txn(&key, kv.create_revision, lease, b"mine")
            .await
            .unwrap());

        // Blind PUT without lease: create_revision is PRESERVED, lease is detached.
        // A create_revision-only guard would wrongly pass; ours must fail (spec §4.4).
        client.put_string(&key, "stolen").await.unwrap();
        assert!(
            !client
                .leader_guard_txn(&key, kv.create_revision, lease, b"mine")
                .await
                .unwrap(),
            "guard must reject an in-place overwrite that preserves create_revision"
        );
    }
}
