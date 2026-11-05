//! Typed operations over the canonical, deadline-bounded Redis execution path.
use super::{backend::RedisBackend, client::RedisLinkClient, resolver::RedisEgressResolver};
use crate::link_sys::LinkSysHealth;
use anyhow::Result;
use redis::{Cmd, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub(crate) enum BoundedGet {
    Missing,
    TooLarge,
    Value(bytes::Bytes),
}
#[derive(Debug)]
pub(crate) enum BoundedGetError {
    Timeout,
    InvalidResponse,
    Dependency(redis::RedisError),
}
fn bounded_get_error(error: redis::RedisError) -> BoundedGetError {
    if error.is_timeout() {
        BoundedGetError::Timeout
    } else {
        BoundedGetError::Dependency(error)
    }
}
fn bounded_get_result(value: Value, limit: usize) -> std::result::Result<BoundedGet, BoundedGetError> {
    let bytes = match value {
        Value::Nil => return Ok(BoundedGet::Missing),
        Value::BulkString(bytes) => bytes,
        Value::SimpleString(s) => s.into_bytes(),
        Value::ServerError(_) => {
            return Err(BoundedGetError::Dependency(redis::RedisError::from((
                redis::ErrorKind::Client,
                "Redis GET returned a command error",
            ))))
        }
        _ => return Err(BoundedGetError::InvalidResponse),
    };
    if bytes.len() > limit {
        return Ok(BoundedGet::TooLarge);
    }
    std::str::from_utf8(&bytes).map_err(|_| BoundedGetError::InvalidResponse)?;
    Ok(BoundedGet::Value(bytes.into()))
}
fn checked_hash_bytes(current: usize, key: usize, value: usize, limit: usize) -> Option<usize> {
    current
        .checked_add(key)?
        .checked_add(value)
        .filter(|total| *total <= limit)
}
fn hash_frame_bytes(value: &Value) -> Result<usize> {
    match value {
        Value::BulkString(bytes) => Ok(bytes.len()),
        Value::SimpleString(s) => Ok(s.len()),
        _ => anyhow::bail!("Redis HGETALL returned a non-string value"),
    }
}
fn bounded_hash_result(value: Value, max_entries: usize, max_bytes: usize) -> Result<Option<HashMap<String, String>>> {
    let mut total = 0;
    match &value {
        Value::Array(values) => {
            anyhow::ensure!(
                values.len() % 2 == 0,
                "Redis HGETALL returned an odd number of elements"
            );
            if values.len() / 2 > max_entries {
                return Ok(None);
            }
            for pair in values.as_chunks::<2>().0 {
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
        Value::Map(values) => {
            if values.len() > max_entries {
                return Ok(None);
            }
            for (key, value) in values {
                let Some(next) = checked_hash_bytes(total, hash_frame_bytes(key)?, hash_frame_bytes(value)?, max_bytes)
                else {
                    return Ok(None);
                };
                total = next;
            }
        }
        _ => anyhow::bail!("Redis HGETALL returned an invalid response"),
    }
    redis::from_redis_value(value)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Redis HGETALL returned an invalid response"))
}
fn operation_error(error: redis::RedisError) -> anyhow::Error {
    anyhow::anyhow!(if error.is_timeout() {
        "Redis operation timed out"
    } else if error.is_io_error() {
        "Redis I/O failed"
    } else {
        "Redis command failed"
    })
}
impl RedisLinkClient {
    async fn query<T: redis::FromRedisValue>(&self, cmd: &Cmd) -> Result<T> {
        let value = self.exec(cmd).await.map_err(operation_error)?;
        redis::from_redis_value(value).map_err(|_| anyhow::anyhow!("Redis returned an invalid response"))
    }
    pub(crate) async fn get_bounded(
        &self,
        key: &str,
        limit: usize,
    ) -> std::result::Result<BoundedGet, BoundedGetError> {
        bounded_get_result(
            self.exec(redis::cmd("GET").arg(key)).await.map_err(bounded_get_error)?,
            limit,
        )
    }
    pub(crate) async fn hgetall_bounded(
        &self,
        key: &str,
        max_entries: usize,
        max_bytes: usize,
    ) -> Result<Option<HashMap<String, String>>> {
        bounded_hash_result(self.exec(redis::cmd("HGETALL").arg(key)).await?, max_entries, max_bytes)
    }
    pub async fn set(&self, key: &str, value: &str, ttl: Option<Duration>) -> Result<()> {
        let mut cmd = redis::cmd("SET");
        cmd.arg(key).arg(value);
        if let Some(ttl) = ttl {
            cmd.arg("EX").arg(ttl.as_secs());
        }
        self.query(&cmd).await
    }
    pub async fn set_px(&self, key: &str, value: &str, ttl_ms: u64) -> Result<()> {
        self.query(redis::cmd("SET").arg(key).arg(value).arg("PX").arg(ttl_ms))
            .await
    }
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        self.query(redis::cmd("GET").arg(key)).await
    }
    pub async fn del(&self, keys: &[&str]) -> Result<u64> {
        self.query(redis::cmd("DEL").arg(keys)).await
    }
    pub async fn exists(&self, key: &str) -> Result<bool> {
        self.query(redis::cmd("EXISTS").arg(key)).await
    }
    pub async fn expire(&self, key: &str, seconds: i64) -> Result<bool> {
        self.query(redis::cmd("EXPIRE").arg(key).arg(seconds)).await
    }
    pub async fn ttl(&self, key: &str) -> Result<i64> {
        self.query(redis::cmd("TTL").arg(key)).await
    }
    pub async fn incr(&self, key: &str) -> Result<i64> {
        self.query(redis::cmd("INCR").arg(key)).await
    }
    pub async fn incr_by(&self, key: &str, increment: i64) -> Result<i64> {
        self.query(redis::cmd("INCRBY").arg(key).arg(increment)).await
    }
    pub async fn hget(&self, key: &str, field: &str) -> Result<Option<String>> {
        self.query(redis::cmd("HGET").arg(key).arg(field)).await
    }
    pub async fn hset(&self, key: &str, field: &str, value: &str) -> Result<u64> {
        self.query(redis::cmd("HSET").arg(key).arg(field).arg(value)).await
    }
    pub async fn hdel(&self, key: &str, fields: &[&str]) -> Result<u64> {
        self.query(redis::cmd("HDEL").arg(key).arg(fields)).await
    }
    pub async fn hgetall(&self, key: &str) -> Result<HashMap<String, String>> {
        self.query(redis::cmd("HGETALL").arg(key)).await
    }
    pub async fn hexists(&self, key: &str, field: &str) -> Result<bool> {
        self.query(redis::cmd("HEXISTS").arg(key).arg(field)).await
    }
    pub async fn rpush(&self, key: &str, values: Vec<String>) -> Result<u64> {
        self.query(redis::cmd("RPUSH").arg(key).arg(values)).await
    }
    pub async fn llen(&self, key: &str) -> Result<u64> {
        self.query(redis::cmd("LLEN").arg(key)).await
    }
    pub async fn lpop(&self, key: &str, count: Option<usize>) -> Result<Vec<String>> {
        let mut cmd = redis::cmd("LPOP");
        cmd.arg(key);
        if let Some(count) = count {
            cmd.arg(count);
        }
        let value = self.exec(&cmd).await?;
        match value {
            Value::Nil => Ok(Vec::new()),
            Value::BulkString(_) | Value::SimpleString(_) => Ok(vec![redis::from_redis_value(value)?]),
            value => Ok(redis::from_redis_value(value)?),
        }
    }
    pub async fn ping(&self) -> Result<u64> {
        let start = std::time::Instant::now();
        let _: String = self.query(&redis::cmd("PING")).await?;
        Ok(start.elapsed().as_millis() as u64)
    }
    pub async fn ping_all_members(&self) -> Result<usize> {
        let resolver = self.resolver.get().ok_or_else(super::backend::unavailable)?.clone();
        let view = self.backend.probe_view(resolver).await;
        let Some((live, configured)) = view.observed else {
            anyhow::bail!("Redis pool PING failed");
        };
        anyhow::ensure!(
            view.healthy && live == configured && configured != 0,
            "Redis pool PING failed"
        );
        Ok(configured)
    }
    pub async fn health_status(&self) -> LinkSysHealth {
        match self.ping().await {
            Ok(latency_ms) => LinkSysHealth {
                name: self.name().to_owned(),
                system_type: "redis".into(),
                connected: true,
                latency_ms: Some(latency_ms),
                error: None,
            },
            Err(_) => LinkSysHealth {
                name: self.name().to_owned(),
                system_type: "redis".into(),
                connected: false,
                latency_ms: None,
                error: Some("Redis PING failed".into()),
            },
        }
    }
}

pub struct LockOptions {
    pub ttl: Duration,
    pub max_wait: Duration,
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
pub struct RedisLockGuard {
    backend: Arc<RedisBackend>,
    resolver: RedisEgressResolver,
    key: String,
    value: String,
    released: bool,
}
const UNLOCK_SCRIPT: &str =
    "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end";
async fn release_lock(backend: &RedisBackend, resolver: RedisEgressResolver, key: &str, value: &str) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + backend.transport.command_timeout;
    let response = backend
        .exec(
            redis::cmd("EVAL").arg(UNLOCK_SCRIPT).arg(1).arg(key).arg(value),
            deadline,
            resolver,
        )
        .await?;
    Ok(redis::from_redis_value::<i64>(response)? == 1)
}
impl RedisLockGuard {
    pub async fn unlock(mut self) -> Result<bool> {
        self.released = true;
        release_lock(&self.backend, self.resolver.clone(), &self.key, &self.value).await
    }
}
impl Drop for RedisLockGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (backend, resolver, key, value) = (
            self.backend.clone(),
            self.resolver.clone(),
            self.key.clone(),
            self.value.clone(),
        );
        runtime.spawn(async move {
            let _ = release_lock(&backend, resolver, &key, &value).await;
        });
    }
}
fn generate_lock_value() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}:{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
impl RedisLinkClient {
    async fn acquire_lock_until(
        &self,
        key: &str,
        ttl: Duration,
        deadline: tokio::time::Instant,
    ) -> Result<Option<RedisLockGuard>> {
        let value = generate_lock_value();
        let result = self
            .exec_until(
                redis::cmd("SET")
                    .arg(key)
                    .arg(&value)
                    .arg("NX")
                    .arg("PX")
                    .arg(ttl.as_millis() as u64),
                deadline,
            )
            .await?;
        if matches!(result, Value::Nil) {
            return Ok(None);
        }
        let _: String = redis::from_redis_value(result)?;
        Ok(Some(RedisLockGuard {
            backend: self.backend.clone(),
            resolver: self.resolver.get().ok_or_else(super::backend::unavailable)?.clone(),
            key: key.to_owned(),
            value,
            released: false,
        }))
    }
    pub async fn try_lock(&self, key: &str, ttl: Duration) -> Result<Option<RedisLockGuard>> {
        self.acquire_lock_until(key, ttl, self.operation_deadline()).await
    }
    pub async fn lock(&self, key: &str, opts: LockOptions) -> Result<RedisLockGuard> {
        let deadline = self
            .operation_deadline()
            .min(tokio::time::Instant::now() + opts.max_wait);
        loop {
            if let Some(guard) = self.acquire_lock_until(key, opts.ttl, deadline).await? {
                return Ok(guard);
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "Redis lock acquisition timed out"
            );
            tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + opts.retry_interval)).await;
        }
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/ops_test.rs"]
mod tests;
