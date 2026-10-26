//! High-level Redis operations.
//!
//! Wraps fred's interface methods with anyhow error handling and Edgion-friendly signatures.
//! Only exposes operations that Edgion actually needs — for advanced use cases,
//! callers can access the underlying pool via `RedisLinkClient::pool()`.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use fred::clients::Pool;
use fred::interfaces::{ClientLike, HashesInterface, KeysInterface, ListInterface, LuaInterface};
use fred::types::{ClusterHash, CustomCommand, MultipleKeys, Resp3Frame, Value as RedisValue};

use super::client::RedisLinkClient;
use crate::link_sys::LinkSysHealth;

fn redis_keys(keys: &[&str]) -> MultipleKeys {
    keys.iter().copied().collect()
}

/// A raw GET result admitted before owned text conversion. Fred has already
/// aggregated the reply; this is not a network-frame memory cap.
pub(crate) enum BoundedGet {
    Missing,
    TooLarge,
    Value(bytes::Bytes),
}
#[derive(Debug)]
pub(crate) enum BoundedGetError {
    Timeout,
    InvalidResponse,
    Dependency(fred::error::Error),
}
fn bounded_get_error(error: fred::error::Error) -> BoundedGetError {
    if matches!(error.kind(), fred::error::ErrorKind::Timeout) {
        BoundedGetError::Timeout
    } else {
        BoundedGetError::Dependency(error)
    }
}
fn bounded_get_result(frame: Resp3Frame, limit: usize) -> std::result::Result<BoundedGet, BoundedGetError> {
    let data = match frame {
        Resp3Frame::Null => return Ok(BoundedGet::Missing),
        Resp3Frame::BlobString { data, .. } | Resp3Frame::SimpleString { data, .. } => data,
        Resp3Frame::SimpleError { .. } | Resp3Frame::BlobError { .. } => {
            // Avoid copying a potentially large/sensitive server error. A raw
            // command-error reply is a dependency failure, not invalid success.
            return Err(BoundedGetError::Dependency(fred::error::Error::new(
                fred::error::ErrorKind::Unknown,
                "Redis GET returned a command error",
            )));
        }
        _ => return Err(BoundedGetError::InvalidResponse),
    };
    if data.len() > limit {
        return Ok(BoundedGet::TooLarge);
    }
    std::str::from_utf8(&data).map_err(|_| BoundedGetError::InvalidResponse)?;
    Ok(BoundedGet::Value(data))
}

// ============================================================================
// Basic Key-Value Operations
// ============================================================================

impl RedisLinkClient {
    pub(crate) async fn get_bounded(
        &self,
        key: &str,
        limit: usize,
    ) -> std::result::Result<BoundedGet, BoundedGetError> {
        let frame = self
            .pool()
            .custom_raw(
                CustomCommand::new_static("GET", ClusterHash::FirstKey, false),
                vec![key],
            )
            .await
            .map_err(bounded_get_error)?;
        bounded_get_result(frame, limit)
    }

    /// GET key → Option<String>
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let val: Option<String> = self
            .pool()
            .get(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis GET {}: {:?}", key, e))?;
        Ok(val)
    }

    /// SET key value [EX seconds]
    pub async fn set(&self, key: &str, value: &str, ttl: Option<Duration>) -> Result<()> {
        use fred::types::Expiration;
        let expiration = ttl.map(|d| Expiration::EX(d.as_secs() as i64));
        self.pool()
            .set::<(), _, _>(key, value, expiration, None, false)
            .await
            .map_err(|e| anyhow::anyhow!("Redis SET {}: {:?}", key, e))?;
        Ok(())
    }

    /// SET key value PX milliseconds (precise TTL)
    pub async fn set_px(&self, key: &str, value: &str, ttl_ms: u64) -> Result<()> {
        use fred::types::Expiration;
        let expiration = Some(Expiration::PX(ttl_ms as i64));
        self.pool()
            .set::<(), _, _>(key, value, expiration, None, false)
            .await
            .map_err(|e| anyhow::anyhow!("Redis SET PX {}: {:?}", key, e))?;
        Ok(())
    }

    /// DEL key(s) — returns number of keys deleted
    pub async fn del(&self, keys: &[&str]) -> Result<u64> {
        let count: u64 = self
            .pool()
            .del(redis_keys(keys))
            .await
            .map_err(|e| anyhow::anyhow!("Redis DEL: {:?}", e))?;
        Ok(count)
    }

    /// EXISTS key — returns true if key exists
    pub async fn exists(&self, key: &str) -> Result<bool> {
        let count: u64 = self
            .pool()
            .exists(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis EXISTS {}: {:?}", key, e))?;
        Ok(count > 0)
    }

    /// EXPIRE key seconds — set TTL on an existing key
    pub async fn expire(&self, key: &str, seconds: i64) -> Result<bool> {
        let result: bool = self
            .pool()
            .expire(key, seconds, None)
            .await
            .map_err(|e| anyhow::anyhow!("Redis EXPIRE {}: {:?}", key, e))?;
        Ok(result)
    }

    /// TTL key → remaining seconds (-1 = no expiry, -2 = key not found)
    pub async fn ttl(&self, key: &str) -> Result<i64> {
        let ttl: i64 = self
            .pool()
            .ttl(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis TTL {}: {:?}", key, e))?;
        Ok(ttl)
    }

    /// INCR key (atomic increment, returns new value)
    pub async fn incr(&self, key: &str) -> Result<i64> {
        let val: i64 = self
            .pool()
            .incr(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis INCR {}: {:?}", key, e))?;
        Ok(val)
    }

    /// INCRBY key increment (atomic increment by amount)
    pub async fn incr_by(&self, key: &str, increment: i64) -> Result<i64> {
        let val: i64 = self
            .pool()
            .incr_by(key, increment)
            .await
            .map_err(|e| anyhow::anyhow!("Redis INCRBY {}: {:?}", key, e))?;
        Ok(val)
    }
}

// ============================================================================
// Hash Operations
// ============================================================================

impl RedisLinkClient {
    /// HGET key field → Option<String>
    pub async fn hget(&self, key: &str, field: &str) -> Result<Option<String>> {
        let val: Option<String> = self
            .pool()
            .hget(key, field)
            .await
            .map_err(|e| anyhow::anyhow!("Redis HGET {} {}: {:?}", key, field, e))?;
        Ok(val)
    }

    /// HSET key field value — returns number of fields added
    pub async fn hset(&self, key: &str, field: &str, value: &str) -> Result<u64> {
        let count: u64 = self
            .pool()
            .hset(key, (field, value))
            .await
            .map_err(|e| anyhow::anyhow!("Redis HSET {} {}: {:?}", key, field, e))?;
        Ok(count)
    }

    /// HDEL key field(s) — returns number of fields removed
    pub async fn hdel(&self, key: &str, fields: &[&str]) -> Result<u64> {
        let count: u64 = self
            .pool()
            .hdel(key, redis_keys(fields))
            .await
            .map_err(|e| anyhow::anyhow!("Redis HDEL {} {:?}: {:?}", key, fields, e))?;
        Ok(count)
    }

    /// HGETALL key → HashMap<String, String>
    pub async fn hgetall(&self, key: &str) -> Result<HashMap<String, String>> {
        let map: HashMap<String, String> = self
            .pool()
            .hgetall(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis HGETALL {}: {:?}", key, e))?;
        Ok(map)
    }

    /// Bound raw hash payload before converting it to owned strings.
    ///
    /// Fred has already aggregated the complete reply; these limits prevent the
    /// subsequent Fred Map and Edgion string-map allocations, not frame buffering.
    /// `None` denotes a count or byte overflow, independently of command errors.
    pub(crate) async fn hgetall_bounded(
        &self,
        key: &str,
        max_entries: usize,
        max_bytes: usize,
    ) -> Result<Option<HashMap<String, String>>> {
        // Reuse the same pool, key routing, timeout, and HGETALL ACL. Fred's custom
        // command forces a flush, trading batching for access before Map conversion.
        let frame = self
            .pool()
            .custom_raw(
                CustomCommand::new_static("HGETALL", ClusterHash::FirstKey, false),
                vec![key],
            )
            .await
            .map_err(|error| anyhow::anyhow!("Redis HGETALL failed: {error}"))?;
        bounded_hash_result(frame, max_entries, max_bytes)
    }

    /// HEXISTS key field → bool
    pub async fn hexists(&self, key: &str, field: &str) -> Result<bool> {
        let exists: bool = self
            .pool()
            .hexists(key, field)
            .await
            .map_err(|e| anyhow::anyhow!("Redis HEXISTS {} {}: {:?}", key, field, e))?;
        Ok(exists)
    }
}

/// Account raw field and value bytes without overflow or partial admission.
fn checked_hash_bytes(current: usize, key_bytes: usize, value_bytes: usize, limit: usize) -> Option<usize> {
    current
        .checked_add(key_bytes)?
        .checked_add(value_bytes)
        .filter(|total| *total <= limit)
}

fn hash_frame_bytes(frame: &Resp3Frame) -> Result<usize> {
    match frame {
        Resp3Frame::BlobString { data, .. }
        | Resp3Frame::SimpleString { data, .. }
        | Resp3Frame::VerbatimString { data, .. }
        | Resp3Frame::BigNumber { data, .. } => Ok(data.len()),
        _ => Err(anyhow::anyhow!("Redis HGETALL returned a non-string value")),
    }
}

fn bounded_hash_result(
    frame: Resp3Frame,
    max_entries: usize,
    max_bytes: usize,
) -> Result<Option<HashMap<String, String>>> {
    let mut total = 0;
    match &frame {
        // Fred represents RESP2's flat HGETALL array as a RESP3 Array.
        Resp3Frame::Array { data, .. } => {
            if data.len() % 2 != 0 {
                return Err(anyhow::anyhow!("Redis HGETALL returned an odd number of elements"));
            }
            if data.len() / 2 > max_entries {
                return Ok(None);
            }
            for pair in data.chunks_exact(2) {
                let Some(next) = checked_hash_bytes(
                    total,
                    hash_frame_bytes(&pair[0])?,
                    hash_frame_bytes(&pair[1])?,
                    max_bytes,
                ) else {
                    return Ok(None);
                };
                total = next;
            }
        }
        Resp3Frame::Map { data, .. } => {
            if data.len() > max_entries {
                return Ok(None);
            }
            for (key, value) in data {
                let Some(next) = checked_hash_bytes(total, hash_frame_bytes(key)?, hash_frame_bytes(value)?, max_bytes)
                else {
                    return Ok(None);
                };
                total = next;
            }
        }
        Resp3Frame::SimpleError { .. } | Resp3Frame::BlobError { .. } => {
            // Preserve Fred's canonical command-error classification.
            RedisValue::try_from(frame)?;
            return Err(anyhow::anyhow!("Redis HGETALL returned an invalid response"));
        }
        _ => return Err(anyhow::anyhow!("Redis HGETALL returned an invalid response")),
    }
    // Only admitted frames reach Fred's canonical conversion. No Map or owned
    // strings are constructed during the borrowed count/byte preflight above.
    let map = RedisValue::try_from(frame)?.into_map()?;
    Ok(Some(RedisValue::Map(map).convert()?))
}

// ============================================================================
// List Operations (for DataSender / queue use cases)
// ============================================================================

impl RedisLinkClient {
    /// RPUSH key value(s) — append to list tail, returns new list length.
    /// Used by DataSender for log buffering.
    pub async fn rpush(&self, key: &str, values: Vec<String>) -> Result<u64> {
        let len: u64 = self
            .pool()
            .rpush(key, values)
            .await
            .map_err(|e| anyhow::anyhow!("Redis RPUSH {}: {:?}", key, e))?;
        Ok(len)
    }

    /// LPOP key [count] — pop from list head
    pub async fn lpop(&self, key: &str, count: Option<usize>) -> Result<Vec<String>> {
        let vals: Vec<String> = self
            .pool()
            .lpop(key, count)
            .await
            .map_err(|e| anyhow::anyhow!("Redis LPOP {}: {:?}", key, e))?;
        Ok(vals)
    }

    /// LLEN key — returns list length
    pub async fn llen(&self, key: &str) -> Result<u64> {
        let len: u64 = self
            .pool()
            .llen(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis LLEN {}: {:?}", key, e))?;
        Ok(len)
    }
}

// ============================================================================
// Distributed Lock (single-instance, SET NX PX)
// ============================================================================

/// Lock guard that auto-releases on drop.
/// Uses a unique value to ensure only the lock holder can release.
pub struct RedisLockGuard {
    pool: Pool,
    key: String,
    value: String,
}

impl RedisLockGuard {
    /// Explicitly release the lock.
    pub async fn unlock(self) -> Result<bool> {
        release_lock(&self.pool, &self.key, &self.value).await
    }
}

impl Drop for RedisLockGuard {
    fn drop(&mut self) {
        // Best-effort release via a spawned task.
        // If the task can't run (runtime shutting down), the lock expires via TTL.
        let pool = self.pool.clone();
        let key = self.key.clone();
        let value = self.value.clone();
        tokio::spawn(async move {
            let _ = release_lock(&pool, &key, &value).await;
        });
    }
}

/// Lua script for atomic lock release:
/// Only delete the key if the value matches (prevents releasing someone else's lock).
const UNLOCK_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("DEL", KEYS[1])
else
    return 0
end
"#;

/// Release lock atomically via Lua EVAL script.
async fn release_lock(pool: &Pool, key: &str, value: &str) -> Result<bool> {
    let result: i64 = pool.eval(UNLOCK_SCRIPT, vec![key], vec![value]).await.unwrap_or(0);
    Ok(result == 1)
}

/// Lock acquisition options.
pub struct LockOptions {
    /// Lock TTL (auto-expire if not released). Default: 10 seconds.
    pub ttl: Duration,
    /// Maximum time to wait for lock acquisition. Default: 5 seconds.
    pub max_wait: Duration,
    /// Retry interval between acquisition attempts. Default: 50ms.
    pub retry_interval: Duration,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(10),
            max_wait: Duration::from_secs(5),
            retry_interval: Duration::from_millis(50),
        }
    }
}

impl RedisLinkClient {
    /// Acquire a distributed lock.
    ///
    /// Uses `SET key value NX PX ttl` for atomic lock acquisition.
    /// Returns a `RedisLockGuard` that auto-releases on drop.
    ///
    /// **Note:** This is a single-instance lock (not full Redlock).
    /// Suitable for leader election and coordination within a single Redis deployment.
    pub async fn lock(&self, key: &str, opts: LockOptions) -> Result<RedisLockGuard> {
        use fred::types::{Expiration, SetOptions};

        let value = generate_lock_value();
        let ttl_ms = opts.ttl.as_millis() as i64;
        let deadline = tokio::time::Instant::now() + opts.max_wait;

        loop {
            let result: Option<String> = self
                .pool()
                .set(
                    key,
                    value.as_str(),
                    Some(Expiration::PX(ttl_ms)),
                    Some(SetOptions::NX),
                    false,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Redis LOCK SET {}: {:?}", key, e))?;

            if result.is_some() {
                return Ok(RedisLockGuard {
                    pool: self.pool().clone(),
                    key: key.to_string(),
                    value,
                });
            }

            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("Redis LOCK {}: timeout after {:?}", key, opts.max_wait);
            }

            tokio::time::sleep(opts.retry_interval).await;
        }
    }

    /// Try to acquire lock without waiting. Returns None if lock is held.
    pub async fn try_lock(&self, key: &str, ttl: Duration) -> Result<Option<RedisLockGuard>> {
        use fred::types::{Expiration, SetOptions};

        let value = generate_lock_value();
        let ttl_ms = ttl.as_millis() as i64;

        let result: Option<String> = self
            .pool()
            .set(
                key,
                value.as_str(),
                Some(Expiration::PX(ttl_ms)),
                Some(SetOptions::NX),
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Redis TRY_LOCK SET {}: {:?}", key, e))?;

        if result.is_some() {
            Ok(Some(RedisLockGuard {
                pool: self.pool().clone(),
                key: key.to_string(),
                value,
            }))
        } else {
            Ok(None)
        }
    }
}

/// Generate a unique lock value for safe release.
/// Format: "pid:timestamp_nanos" — unique across processes and time.
fn generate_lock_value() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}:{}", std::process::id(), ts.as_nanos())
}

// ============================================================================
// Health Check
// ============================================================================

impl RedisLinkClient {
    /// Active health check via PING. Returns latency in milliseconds.
    pub async fn ping(&self) -> Result<u64> {
        let start = std::time::Instant::now();
        self.pool()
            .ping::<String>(None)
            .await
            .map_err(|e| anyhow::anyhow!("Redis PING failed: {:?}", e))?;
        Ok(std::time::Instant::now().saturating_duration_since(start).as_millis() as u64)
    }

    /// Get detailed health status for admin API exposure.
    pub async fn health_status(&self) -> LinkSysHealth {
        match self.ping().await {
            Ok(latency_ms) => LinkSysHealth {
                name: self.name().to_string(),
                system_type: "redis".to_string(),
                connected: true,
                latency_ms: Some(latency_ms),
                error: None,
            },
            Err(e) => LinkSysHealth {
                name: self.name().to_string(),
                system_type: "redis".to_string(),
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

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/ops_test.rs"]
mod tests;
