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

use super::client::RedisLinkClient;

// ============================================================================
// Basic Key-Value Operations
// ============================================================================

impl RedisLinkClient {
    /// GET key → Option<String>
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let val: Option<String> = self
            .pool()
            .get(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis GET {}: {:?}", key, e))?;
        Ok(val)
    }

    /// GET key → Option<Vec<u8>> (binary safe)
    pub async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let val: Option<Vec<u8>> = self
            .pool()
            .get(key)
            .await
            .map_err(|e| anyhow::anyhow!("Redis GET bytes {}: {:?}", key, e))?;
        Ok(val)
    }

    /// SET key value [EX seconds]
    pub async fn set(
        &self,
        key: &str,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<()> {
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
            .del(keys.to_vec())
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
            .hdel(key, fields.to_vec())
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

    /// LTRIM key start stop — trim list to [start, stop] range
    pub async fn ltrim(&self, key: &str, start: i64, stop: i64) -> Result<()> {
        self.pool()
            .ltrim::<(), _>(key, start, stop)
            .await
            .map_err(|e| anyhow::anyhow!("Redis LTRIM {}: {:?}", key, e))?;
        Ok(())
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
    let result: i64 = pool
        .eval(UNLOCK_SCRIPT, vec![key], vec![value])
        .await
        .unwrap_or(0);
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
    pub async fn try_lock(
        &self,
        key: &str,
        ttl: Duration,
    ) -> Result<Option<RedisLockGuard>> {
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
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}:{}", std::process::id(), ts.as_nanos())
}

// ============================================================================
// Health Check
// ============================================================================

use serde::Serialize;

/// Health status of a LinkSys client, exposed via admin API.
#[derive(Debug, Clone, Serialize)]
pub struct LinkSysHealth {
    pub name: String,
    pub system_type: String,
    pub connected: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

impl RedisLinkClient {
    /// Active health check via PING. Returns latency in milliseconds.
    pub async fn ping(&self) -> Result<u64> {
        let start = std::time::Instant::now();
        self.pool()
            .ping::<String>(None)
            .await
            .map_err(|e| anyhow::anyhow!("Redis PING failed: {:?}", e))?;
        Ok(start.elapsed().as_millis() as u64)
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
mod tests {
    use super::*;

    #[test]
    fn test_generate_lock_value_is_unique() {
        let v1 = generate_lock_value();
        std::thread::sleep(std::time::Duration::from_nanos(1));
        let v2 = generate_lock_value();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_lock_options_default() {
        let opts = LockOptions::default();
        assert_eq!(opts.ttl, Duration::from_secs(10));
        assert_eq!(opts.max_wait, Duration::from_secs(5));
        assert_eq!(opts.retry_interval, Duration::from_millis(50));
    }
}
