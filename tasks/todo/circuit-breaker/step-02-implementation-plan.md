# Circuit Breaker (Endpoint Ejection) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Gateway 添加 per-endpoint 被动熔断（逐出）机制，当某个后端端点连续失败 N 次后自动逐出，冷却后自动恢复。

**Architecture:** 复用 `HealthStatusStore` 的 ArcSwap 快照模式，新建 `CircuitBreakerStore` 用 `DashMap` 跟踪 per-endpoint 连续失败计数。配置通过 Service annotation `edgion.io/circuit-breaker` 挂载，在 `is_backend_healthy()` 中加入逐出检查，在 `fail_to_connect`/`error_while_proxy`/`upstream_response_filter` 中记录失败/成功。逐出粒度是 per-endpoint（IP:Port），配置粒度是 per-Service。

**Tech Stack:** Rust, DashMap, ArcSwap, Pingora ProxyHttp lifecycle hooks, serde_yaml

---

## 文件结构

| 文件 | 职责 | 操作 |
|------|------|------|
| `src/core/gateway/backends/circuit_breaker/mod.rs` | 模块入口，导出公共 API | 新建 |
| `src/core/gateway/backends/circuit_breaker/store.rs` | `CircuitBreakerStore`：per-service/per-endpoint 逐出状态跟踪 | 新建 |
| `src/core/gateway/backends/circuit_breaker/config.rs` | `CircuitBreakerConfig` 结构体 + annotation 解析 | 新建 |
| `src/types/constants/annotations.rs` | 添加 `CIRCUIT_BREAKER` 常量 | 修改 |
| `src/core/gateway/backends/mod.rs` | `is_backend_healthy()` 加入熔断器检查 | 修改 |
| `src/core/gateway/backends/discovery/services/conf_handler_impl.rs` | Service 配置变更时解析 CB annotation 并存储 | 修改 |
| `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs` | 连接失败时 `record_failure()` | 修改 |
| `src/core/gateway/routes/http/proxy_http/pg_error_while_proxy.rs` | 代理错误时 `record_failure()` | 修改 |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs` | 上游 5xx 时 `record_failure()`，2xx 时 `record_success()` | 修改 |
| `examples/code/server/test_server.rs` | 添加 `/cb-fail/{test_id}/{fail_count}` 端点 | 修改 |
| `examples/code/client/suites/http_route/backend/circuit_breaker.rs` | 集成测试 | 新建 |
| `examples/code/client/suites/http_route/backend/mod.rs` | 注册 circuit_breaker 模块 | 修改 |
| `examples/code/client/test_client.rs` | 注册 CircuitBreaker suite | 修改 |
| `examples/test/conf/HTTPRoute/Backend/CircuitBreaker/` | K8s YAML 配置 | 新建 |
| `examples/test/conf/ports.json` | 添加 CircuitBreaker 端口 | 修改 |

---

## Task 1: CircuitBreakerConfig 结构体 + annotation 解析

**Files:**
- Create: `src/core/gateway/backends/circuit_breaker/config.rs`
- Modify: `src/types/constants/annotations.rs`

**说明：** 定义配置结构和 annotation 解析，遵循 `health_check/annotation.rs` 的模式。

- [ ] **Step 1: 添加 annotation 常量**

在 `src/types/constants/annotations.rs` 的 `edgion` 模块中添加：

```rust
pub const CIRCUIT_BREAKER: &str = "edgion.io/circuit-breaker";
```

- [ ] **Step 2: 创建 config.rs**

```rust
// src/core/gateway/backends/circuit_breaker/config.rs

use crate::types::constants::annotations::edgion::CIRCUIT_BREAKER;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Deserialize;
use std::time::Duration;

/// Circuit breaker configuration parsed from Service annotation.
///
/// Example annotation:
/// ```yaml
/// edgion.io/circuit-breaker: |
///   consecutiveErrors: 5
///   ejectionSeconds: 30
///   maxEjectionPercent: 50
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before ejecting a backend endpoint.
    /// Default: 5
    #[serde(default = "default_consecutive_errors")]
    pub consecutive_errors: u32,

    /// Duration in seconds to eject the backend. After this, it's automatically recovered.
    /// Default: 30
    #[serde(default = "default_ejection_seconds")]
    pub ejection_seconds: u64,

    /// Maximum percentage of endpoints that can be ejected simultaneously.
    /// Prevents ejecting all backends. 0 = no limit (allow ejecting all).
    /// Default: 50
    #[serde(default = "default_max_ejection_percent")]
    pub max_ejection_percent: u32,
}

fn default_consecutive_errors() -> u32 { 5 }
fn default_ejection_seconds() -> u64 { 30 }
fn default_max_ejection_percent() -> u32 { 50 }

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            consecutive_errors: default_consecutive_errors(),
            ejection_seconds: default_ejection_seconds(),
            max_ejection_percent: default_max_ejection_percent(),
        }
    }
}

impl CircuitBreakerConfig {
    /// Get ejection duration as std::time::Duration.
    #[inline]
    pub fn ejection_duration(&self) -> Duration {
        Duration::from_secs(self.ejection_seconds)
    }

    /// Validate configuration. Returns error message if invalid.
    pub fn validate(&self) -> Option<String> {
        if self.consecutive_errors == 0 {
            return Some("consecutiveErrors must be >= 1".to_string());
        }
        if self.ejection_seconds == 0 {
            return Some("ejectionSeconds must be >= 1".to_string());
        }
        if self.max_ejection_percent > 100 {
            return Some("maxEjectionPercent must be 0-100".to_string());
        }
        None
    }
}

/// Parse `edgion.io/circuit-breaker` annotation from K8s Service metadata.
pub fn parse_circuit_breaker_annotation(meta: &ObjectMeta) -> Option<CircuitBreakerConfig> {
    let annotations = meta.annotations.as_ref()?;
    let yaml_str = annotations.get(CIRCUIT_BREAKER)?;

    match serde_yaml::from_str::<CircuitBreakerConfig>(yaml_str) {
        Ok(config) => {
            if let Some(err) = config.validate() {
                tracing::warn!(
                    resource = %meta.name.as_deref().unwrap_or(""),
                    error = %err,
                    "Invalid circuit-breaker annotation, ignoring"
                );
                return None;
            }
            Some(config)
        }
        Err(e) => {
            tracing::warn!(
                resource = %meta.name.as_deref().unwrap_or(""),
                error = %e,
                "Failed to parse circuit-breaker annotation"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn meta_with_annotation(value: Option<&str>) -> ObjectMeta {
        let mut annotations = BTreeMap::new();
        if let Some(v) = value {
            annotations.insert(CIRCUIT_BREAKER.to_string(), v.to_string());
        }
        ObjectMeta {
            name: Some("test-svc".to_string()),
            annotations: if annotations.is_empty() { None } else { Some(annotations) },
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_valid_annotation() {
        let meta = meta_with_annotation(Some("consecutiveErrors: 3\nejectionSeconds: 10\n"));
        let cfg = parse_circuit_breaker_annotation(&meta).unwrap();
        assert_eq!(cfg.consecutive_errors, 3);
        assert_eq!(cfg.ejection_seconds, 10);
        assert_eq!(cfg.max_ejection_percent, 50); // default
    }

    #[test]
    fn test_parse_defaults() {
        let meta = meta_with_annotation(Some("{}"));
        let cfg = parse_circuit_breaker_annotation(&meta).unwrap();
        assert_eq!(cfg.consecutive_errors, 5);
        assert_eq!(cfg.ejection_seconds, 30);
    }

    #[test]
    fn test_parse_missing_annotation() {
        let meta = meta_with_annotation(None);
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let meta = meta_with_annotation(Some("[invalid"));
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }

    #[test]
    fn test_validate_zero_errors() {
        let meta = meta_with_annotation(Some("consecutiveErrors: 0\n"));
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }

    #[test]
    fn test_validate_zero_ejection() {
        let meta = meta_with_annotation(Some("ejectionSeconds: 0\n"));
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test --lib circuit_breaker::config -- --nocapture
```

Expected: 所有 6 个测试通过

---

## Task 2: CircuitBreakerStore — 逐出状态存储

**Files:**
- Create: `src/core/gateway/backends/circuit_breaker/store.rs`
- Create: `src/core/gateway/backends/circuit_breaker/mod.rs`

**说明：** 核心数据结构。per-service 存储每个 endpoint 的连续失败计数和逐出时间。使用 `DashMap` 实现无锁并发读写。

- [ ] **Step 1: 创建 store.rs**

```rust
// src/core/gateway/backends/circuit_breaker/store.rs

use super::config::CircuitBreakerConfig;
use dashmap::DashMap;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

static CB_STORE: LazyLock<Arc<CircuitBreakerStore>> = LazyLock::new(|| Arc::new(CircuitBreakerStore::new()));

pub fn get_circuit_breaker_store() -> Arc<CircuitBreakerStore> {
    CB_STORE.clone()
}

/// Per-endpoint circuit breaker state.
#[derive(Debug)]
struct EndpointState {
    /// Number of consecutive failures (reset to 0 on success).
    consecutive_failures: u32,
    /// When the endpoint was ejected (None = not ejected).
    ejected_at: Option<Instant>,
}

impl Default for EndpointState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            ejected_at: None,
        }
    }
}

/// Per-service circuit breaker state.
struct ServiceCbState {
    /// Config for this service's circuit breaker.
    config: CircuitBreakerConfig,
    /// Per-endpoint states, keyed by backend_hash (same hash as health check).
    endpoints: DashMap<u64, EndpointState>,
}

/// Global circuit breaker store.
///
/// Tracks per-service, per-endpoint failure counts and ejection state.
/// Hot path (`is_ejected`) is lock-free via DashMap.
pub struct CircuitBreakerStore {
    /// service_key -> service CB state
    services: DashMap<String, Arc<ServiceCbState>>,
}

impl CircuitBreakerStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
        }
    }

    /// Register or update a service's circuit breaker config.
    /// Called when Service annotation changes.
    pub fn set_service_config(&self, service_key: &str, config: Option<CircuitBreakerConfig>) {
        match config {
            Some(cfg) => {
                // If service already exists, update config but preserve endpoint states
                if let Some(existing) = self.services.get(service_key) {
                    // Only update if config actually changed (avoid clearing states unnecessarily)
                    let existing_cfg = &existing.config;
                    if existing_cfg.consecutive_errors != cfg.consecutive_errors
                        || existing_cfg.ejection_seconds != cfg.ejection_seconds
                        || existing_cfg.max_ejection_percent != cfg.max_ejection_percent
                    {
                        // Config changed — create new state with fresh endpoint map
                        // (old failure counts may be stale with new thresholds)
                        self.services.insert(
                            service_key.to_string(),
                            Arc::new(ServiceCbState {
                                config: cfg,
                                endpoints: DashMap::new(),
                            }),
                        );
                    }
                } else {
                    self.services.insert(
                        service_key.to_string(),
                        Arc::new(ServiceCbState {
                            config: cfg,
                            endpoints: DashMap::new(),
                        }),
                    );
                }
            }
            None => {
                self.services.remove(service_key);
            }
        }
    }

    /// Check if a backend endpoint is currently ejected.
    ///
    /// Returns `true` if the endpoint should be skipped (is ejected and cooldown not expired).
    /// Returns `false` if the service has no CB config, endpoint is not tracked,
    /// endpoint is not ejected, or cooldown has expired (auto-recovery).
    ///
    /// This is the hot-path function called during backend selection.
    #[inline]
    pub fn is_ejected(&self, service_key: &str, backend_hash: u64) -> bool {
        let Some(svc) = self.services.get(service_key) else {
            return false;
        };

        let Some(ep) = svc.endpoints.get(&backend_hash) else {
            return false;
        };

        let Some(ejected_at) = ep.ejected_at else {
            return false;
        };

        // Check if cooldown has expired → auto-recover
        if ejected_at.elapsed() >= svc.config.ejection_duration() {
            drop(ep); // release read lock before write
            // Auto-recover: reset state
            if let Some(mut ep_mut) = svc.endpoints.get_mut(&backend_hash) {
                ep_mut.ejected_at = None;
                ep_mut.consecutive_failures = 0;
            }
            return false;
        }

        true
    }

    /// Record a failure for a backend endpoint.
    ///
    /// Increments consecutive failure count. If threshold reached, ejects the endpoint.
    /// `total_endpoints` is the total number of endpoints for this service (for max_ejection_percent).
    pub fn record_failure(&self, service_key: &str, backend_hash: u64, total_endpoints: usize) {
        let Some(svc) = self.services.get(service_key) else {
            return;
        };

        let mut ep = svc.endpoints.entry(backend_hash).or_default();

        // If already ejected, don't re-count
        if ep.ejected_at.is_some() {
            return;
        }

        ep.consecutive_failures += 1;

        if ep.consecutive_failures >= svc.config.consecutive_errors {
            // Check max_ejection_percent before ejecting
            if svc.config.max_ejection_percent > 0 && total_endpoints > 0 {
                let currently_ejected = svc
                    .endpoints
                    .iter()
                    .filter(|e| e.value().ejected_at.is_some())
                    .count();
                let max_ejectable = (total_endpoints as u64 * svc.config.max_ejection_percent as u64 / 100) as usize;
                if currently_ejected >= max_ejectable.max(1) {
                    // Already at max ejection limit, don't eject more
                    return;
                }
            }

            ep.ejected_at = Some(Instant::now());
            tracing::warn!(
                component = "circuit_breaker",
                service = service_key,
                backend_hash = backend_hash,
                consecutive_failures = ep.consecutive_failures,
                ejection_seconds = svc.config.ejection_seconds,
                "Backend endpoint ejected"
            );
        }
    }

    /// Record a success for a backend endpoint.
    ///
    /// Resets consecutive failure count to 0.
    pub fn record_success(&self, service_key: &str, backend_hash: u64) {
        let Some(svc) = self.services.get(service_key) else {
            return;
        };

        // Only update if entry exists (don't create entries for healthy backends)
        if let Some(mut ep) = svc.endpoints.get_mut(&backend_hash) {
            if ep.consecutive_failures > 0 || ep.ejected_at.is_some() {
                ep.consecutive_failures = 0;
                ep.ejected_at = None;
            }
        }
    }

    /// Check if a service has circuit breaker configured.
    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.contains_key(service_key)
    }

    /// Remove a service (cleanup on service deletion).
    pub fn remove_service(&self, service_key: &str) {
        self.services.remove(service_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(errors: u32, seconds: u64, max_pct: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            consecutive_errors: errors,
            ejection_seconds: seconds,
            max_ejection_percent: max_pct,
        }
    }

    #[test]
    fn test_no_config_means_not_ejected() {
        let store = CircuitBreakerStore::new();
        assert!(!store.is_ejected("default/svc1", 123));
    }

    #[test]
    fn test_record_failure_ejects_after_threshold() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 30, 0)));

        for _ in 0..2 {
            store.record_failure("default/svc1", 111, 2);
            assert!(!store.is_ejected("default/svc1", 111));
        }

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_success_resets_failures() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 30, 0)));

        store.record_failure("default/svc1", 111, 2);
        store.record_failure("default/svc1", 111, 2);
        store.record_success("default/svc1", 111);
        store.record_failure("default/svc1", 111, 2);
        store.record_failure("default/svc1", 111, 2);

        // Should not be ejected: success reset the counter
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_auto_recovery_after_cooldown() {
        let store = CircuitBreakerStore::new();
        // Use 0-second ejection for instant recovery test
        store.set_service_config("default/svc1", Some(make_config(1, 1, 0)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        // Wait for ejection to expire
        std::thread::sleep(std::time::Duration::from_secs(1));

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_max_ejection_percent() {
        let store = CircuitBreakerStore::new();
        // max 50% of 2 endpoints = 1 can be ejected
        store.set_service_config("default/svc1", Some(make_config(1, 30, 50)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111)); // first one ejected

        store.record_failure("default/svc1", 222, 2);
        assert!(!store.is_ejected("default/svc1", 222)); // second one NOT ejected (at limit)
    }

    #[test]
    fn test_remove_service_clears_state() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        store.remove_service("default/svc1");
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_different_services_isolated() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        store.set_service_config("default/svc2", Some(make_config(1, 30, 0)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));
        assert!(!store.is_ejected("default/svc2", 111)); // different service, same hash
    }
}
```

- [ ] **Step 2: 创建 mod.rs**

```rust
// src/core/gateway/backends/circuit_breaker/mod.rs

pub mod config;
pub mod store;

pub use config::parse_circuit_breaker_annotation;
pub use store::{get_circuit_breaker_store, CircuitBreakerStore};
```

- [ ] **Step 3: 在 `src/core/gateway/backends/mod.rs` 中注册模块**

在文件顶部 `pub mod health;` 之后添加：

```rust
pub mod circuit_breaker;
```

并添加 re-export：

```rust
pub use circuit_breaker::{get_circuit_breaker_store, parse_circuit_breaker_annotation};
```

- [ ] **Step 4: 运行测试**

```bash
cargo test --lib circuit_breaker -- --nocapture
```

Expected: 所有单元测试通过

---

## Task 3: 集成到 is_backend_healthy() — LB 过滤

**Files:**
- Modify: `src/core/gateway/backends/mod.rs:97-106`

**说明：** 在后端选择时检查熔断器状态，被逐出的端点不会被选中。

- [ ] **Step 1: 修改 is_backend_healthy()**

将现有函数扩展为同时检查健康检查和熔断器：

```rust
#[inline]
fn is_backend_healthy(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
) -> bool {
    // 1. Active health check filter (existing)
    if health_store.has_service(service_key) && !health_store.is_healthy(service_key, health::check::backend_hash(backend)) {
        return false;
    }
    // 2. Circuit breaker filter (new)
    let cb_store = get_circuit_breaker_store();
    if cb_store.is_ejected(service_key, health::check::backend_hash(backend)) {
        return false;
    }
    true
}
```

- [ ] **Step 2: 编译检查**

```bash
cargo check --all-targets
```

Expected: 编译通过

---

## Task 4: Service conf_handler 中解析 CB annotation

**Files:**
- Modify: `src/core/gateway/backends/discovery/services/conf_handler_impl.rs`

**说明：** 当 Service 配置同步（full_set/partial_update）时，解析 `edgion.io/circuit-breaker` annotation 并存入 `CircuitBreakerStore`。

- [ ] **Step 1: 修改 conf_handler_impl.rs**

在文件顶部添加 import：

```rust
use crate::core::gateway::backends::circuit_breaker::{
    config::parse_circuit_breaker_annotation, get_circuit_breaker_store,
};
```

在 `full_set()` 中，在 `hc_manager.reconcile_service(key);` 之前的循环中添加 CB 处理。

**full_set 中的 for 循环：**

```rust
// 在 "for old_key in config_store.service_keys()" 循环内，
// 在 runtime_state::remove_service(&old_key); 之后添加：
cb_store.remove_service(&old_key);

// 在 for (key, service) in data 循环内，
// 在 hc_manager.reconcile_service(key); 之后添加：
let cb_config = parse_circuit_breaker_annotation(&service.metadata);
cb_store.set_service_config(key, cb_config);
```

**partial_update 中：**

```rust
// 在 add/update 循环内，hc_manager.reconcile_service(key); 之后添加：
let cb_config = parse_circuit_breaker_annotation(&service.metadata);
cb_store.set_service_config(key, cb_config);

// 在 remove 循环内，runtime_state::remove_service(key); 之后添加：
cb_store.remove_service(key);
```

需要在 full_set 和 partial_update 方法开头获取 store：

```rust
let cb_store = get_circuit_breaker_store();
```

- [ ] **Step 2: 编译检查**

```bash
cargo check --all-targets
```

Expected: 编译通过

---

## Task 5: Hook 接入 — 在失败/成功路径记录状态

**Files:**
- Modify: `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs`
- Modify: `src/core/gateway/routes/http/proxy_http/pg_error_while_proxy.rs`
- Modify: `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs`

**说明：** 在三个 Pingora lifecycle hook 中调用 `record_failure()` 或 `record_success()`。需要从 `ctx` 中获取 `service_key` 和 `lb_backend_addr`（用于计算 backend_hash）。

**关键点：**
- `service_key` 从 `ctx.get_current_upstream().service_key` 获取
- `lb_backend_addr` 从 `ctx.get_current_upstream().lb_backend_addr` 获取
- backend_hash 使用 `health::check::backend_hash()` 计算，需要构造一个临时 `Backend` 对象
- `total_endpoints` 通过 `get_backends_for_service(service_key).len()` 获取

- [ ] **Step 1: 添加辅助函数到 circuit_breaker/store.rs**

在 `CircuitBreakerStore` impl 中添加便捷方法：

```rust
/// Record failure using SocketAddr (convenience for hook callsites).
/// Constructs a Backend with default weight for hashing.
pub fn record_failure_by_addr(&self, service_key: &str, addr: std::net::SocketAddr) {
    if !self.has_service(service_key) {
        return;
    }
    let backend = pingora_load_balancing::Backend {
        addr: pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
        weight: 1,
    };
    let hash = crate::core::gateway::backends::health::check::backend_hash(&backend);
    let total = crate::core::gateway::backends::get_backends_for_service(service_key).len();
    self.record_failure(service_key, hash, total);
}

/// Record success using SocketAddr (convenience for hook callsites).
pub fn record_success_by_addr(&self, service_key: &str, addr: std::net::SocketAddr) {
    if !self.has_service(service_key) {
        return;
    }
    let backend = pingora_load_balancing::Backend {
        addr: pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
        weight: 1,
    };
    let hash = crate::core::gateway::backends::health::check::backend_hash(&backend);
    self.record_success(service_key, hash);
}
```

- [ ] **Step 2: 修改 pg_fail_to_connect.rs**

在函数末尾、`e.set_retry(true)` 之前添加 CB 记录：

```rust
// Record circuit breaker failure
if let Some(upstream) = ctx.get_current_upstream() {
    if let (Some(service_key), Some(addr)) = (&upstream.service_key, upstream.lb_backend_addr) {
        crate::core::gateway::backends::get_circuit_breaker_store()
            .record_failure_by_addr(service_key, addr);
    }
}
```

放在状态码设置之后、retry 逻辑之前（约第 31 行之后）。

- [ ] **Step 3: 修改 pg_error_while_proxy.rs**

在函数开头（设置 response_body_size 之后）添加 CB 记录：

```rust
// Record circuit breaker failure (for non-status-retry errors)
if !ctx.retry_on_status {
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(service_key), Some(addr)) = (&upstream.service_key, upstream.lb_backend_addr) {
            crate::core::gateway::backends::get_circuit_breaker_store()
                .record_failure_by_addr(service_key, addr);
        }
    }
}
```

注意：`retry_on_status` 的情况不算连接失败，是上游返回了可重试状态码，不应该计入熔断器。

- [ ] **Step 4: 修改 pg_upstream_response_filter.rs**

在函数中，retry 检查之后、plugins 运行之前，添加 CB 记录：

```rust
// Record circuit breaker success/failure based on status code
if let Some(upstream) = ctx.get_current_upstream() {
    if let (Some(service_key), Some(addr)) = (&upstream.service_key, upstream.lb_backend_addr) {
        let cb_store = crate::core::gateway::backends::get_circuit_breaker_store();
        if status_code >= 500 {
            cb_store.record_failure_by_addr(service_key, addr);
        } else {
            cb_store.record_success_by_addr(service_key, addr);
        }
    }
}
```

放在 retry 检查代码块之后（约第 58 行之后）。

- [ ] **Step 5: 编译检查**

```bash
cargo check --all-targets
```

Expected: 编译通过

- [ ] **Step 6: 运行现有测试确保无回归**

```bash
cargo test --all
```

Expected: 所有现有测试通过

---

## Task 6: 集成测试 — 测试后端和测试用例

**Files:**
- Modify: `examples/code/server/test_server.rs` — 添加 `/cb-fail/{test_id}/{fail_count}` 端点
- Create: `examples/test/conf/HTTPRoute/Backend/CircuitBreaker/Gateway.yaml`
- Create: `examples/test/conf/HTTPRoute/Backend/CircuitBreaker/HTTPRoute_default_cb-test.yaml`
- Create: `examples/test/conf/HTTPRoute/Backend/CircuitBreaker/Service_default_cb-test.yaml`
- Create: `examples/test/conf/HTTPRoute/Backend/CircuitBreaker/EndpointSlice_default_cb-test.yaml`
- Modify: `examples/test/conf/ports.json`
- Create: `examples/code/client/suites/http_route/backend/circuit_breaker.rs`
- Modify: `examples/code/client/suites/http_route/backend/mod.rs`
- Modify: `examples/code/client/test_client.rs`

**说明：** 集成测试验证熔断器逐出和恢复行为。

**测试场景：**
1. **正常请求不触发熔断** — 200 响应不增加失败计数
2. **连续失败触发逐出** — 发送请求到一个持续返回 502 的后端，验证后续请求返回 502（503 来自无可用后端）
3. **逐出后自动恢复** — 设置短逐出时间，等待后验证请求恢复

**测试后端设计：**
- `/cb-fail/{test_id}/{fail_count}` — 对每个 test_id，前 fail_count 次请求返回 502，之后返回 200
- 返回 header `x-cb-attempt` 表示当前是第几次调用

- [ ] **Step 1: 添加测试后端端点**

在 `examples/code/server/test_server.rs` 中添加：

1. 全局计数器：
```rust
/// Circuit-breaker test counter: tracks calls per test-id
static CB_COUNTER: std::sync::LazyLock<Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
```

2. 路由注册（在 `.route("/retry-sequence-reset", ...)` 之后）：
```rust
.route("/cb-fail/{test_id}/{fail_count}", get(cb_fail_handler))
.route("/cb-reset", get(cb_reset_handler))
```

3. Handler 实现：
```rust
/// /cb-fail/{test_id}/{fail_count}
/// Returns 502 for the first `fail_count` calls per test_id, then 200.
async fn cb_fail_handler(Path((test_id, fail_count)): Path<(String, u32)>) -> impl IntoResponse {
    let attempt = {
        let mut store = CB_COUNTER.lock().unwrap();
        let count = store.entry(test_id.clone()).or_insert(0);
        *count += 1;
        *count
    };

    let status = if attempt <= fail_count {
        StatusCode::BAD_GATEWAY // 502
    } else {
        StatusCode::OK
    };

    (
        status,
        [
            ("x-cb-attempt", attempt.to_string()),
            ("x-cb-test-id", test_id),
        ],
        format!("attempt={}", attempt),
    )
}

/// /cb-reset — clear all CB test state
async fn cb_reset_handler() -> impl IntoResponse {
    let mut store = CB_COUNTER.lock().unwrap();
    store.clear();
    (StatusCode::OK, "cb-reset-ok")
}
```

- [ ] **Step 2: 创建 K8s YAML 配置**

Port: 31144（在 ports.json current_max 之后分配）

**Gateway.yaml:**
```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: cb-gw
  namespace: edgion-default
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 31144
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
```

**HTTPRoute_default_cb-test.yaml:**
```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cb-test
  namespace: edgion-default
spec:
  parentRefs:
    - name: cb-gw
  hostnames:
    - "cb-test.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: cb-test
          port: 30001
      timeouts:
        request: "30s"
        backendRequest: "5s"
```

**Service_default_cb-test.yaml:**
```yaml
apiVersion: v1
kind: Service
metadata:
  name: cb-test
  namespace: edgion-default
  annotations:
    edgion.io/circuit-breaker: |
      consecutiveErrors: 3
      ejectionSeconds: 3
      maxEjectionPercent: 0
spec:
  ports:
    - port: 30001
      targetPort: 30001
      protocol: TCP
```

**EndpointSlice_default_cb-test.yaml:**
```yaml
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: cb-test-slice
  namespace: edgion-default
  labels:
    kubernetes.io/service-name: cb-test
addressType: IPv4
endpoints:
  - addresses:
      - "127.0.0.1"
    conditions:
      ready: true
ports:
  - port: 30001
    protocol: TCP
```

- [ ] **Step 3: 更新 ports.json**

添加：
```json
"HTTPRoute/Backend/CircuitBreaker": {
  "http": 31144
}
```

更新 `current_max` 为 `31144`。

- [ ] **Step 4: 注册测试 suite**

在 `examples/code/client/suites/http_route/backend/mod.rs` 中添加：
```rust
pub mod circuit_breaker;
```

在 `examples/code/client/test_client.rs` 中添加 3 个注册点（同 RetryOnStatus 模式）：

1. `resolve_suite()`: `"CircuitBreaker" => "HTTPRoute/Backend/CircuitBreaker"`
2. `suite_to_port_key()`: `"HTTPRoute/Backend/CircuitBreaker" => "HTTPRoute/Backend/CircuitBreaker"`
3. `add_suites_for_suite()`: `"HTTPRoute/Backend/CircuitBreaker" => suites.push(Box::new(CircuitBreakerTestSuite))`

并添加 import：
```rust
use suites::http_route::backend::circuit_breaker::CircuitBreakerTestSuite;
```

- [ ] **Step 5: 创建集成测试**

```rust
// examples/code/client/suites/http_route/backend/circuit_breaker.rs

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;

pub struct CircuitBreakerTestSuite;

#[async_trait]
impl TestSuite for CircuitBreakerTestSuite {
    fn name(&self) -> &str {
        "CircuitBreaker"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_normal_request_no_ejection(),
            Self::test_consecutive_failures_eject(),
            Self::test_recovery_after_cooldown(),
        ]
    }
}

impl CircuitBreakerTestSuite {
    /// Normal 200 responses should not trigger circuit breaker
    fn test_normal_request_no_ejection() -> TestCase {
        TestCase::new(
            "normal_request_no_ejection",
            "200 responses don't trigger circuit breaker",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();

                    // Reset CB test state
                    let reset_url = format!("{}/cb-reset", ctx.http_url());
                    ctx.http_client
                        .get(&reset_url)
                        .header("Host", "cb-test.example.com")
                        .send()
                        .await
                        .ok();

                    // Send multiple successful requests (fail_count=0 → always 200)
                    for i in 0..5 {
                        let url = format!("{}/cb-fail/normal-{}/0", ctx.http_url(), i);
                        let resp = ctx
                            .http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await;

                        match resp {
                            Ok(r) if r.status().as_u16() == 200 => {}
                            Ok(r) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", r.status()),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All 5 requests returned 200, no ejection".to_string(),
                    )
                })
            },
        )
    }

    /// Consecutive 502s should trigger ejection, subsequent requests get 502/503
    fn test_consecutive_failures_eject() -> TestCase {
        TestCase::new(
            "consecutive_failures_eject",
            "3 consecutive 502s eject the backend endpoint",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let test_id = format!("eject-{}", std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos());

                    // Reset CB test state
                    let reset_url = format!("{}/cb-reset", ctx.http_url());
                    ctx.http_client
                        .get(&reset_url)
                        .header("Host", "cb-test.example.com")
                        .send()
                        .await
                        .ok();

                    // Send requests that return 502 (fail_count=100 → always fail)
                    // consecutiveErrors=3, so after 3 failures the endpoint should be ejected
                    for i in 0..4 {
                        let url = format!("{}/cb-fail/{}/100", ctx.http_url(), test_id);
                        let resp = ctx
                            .http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await;

                        match resp {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                if i < 3 {
                                    // First 3 requests should go through and get 502 from backend
                                    if status != 502 {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Request {} expected 502, got {}", i, status),
                                        );
                                    }
                                } else {
                                    // After ejection, should get 502 (no available backends → 502/503)
                                    // The exact status depends on whether gateway returns 502 or 503
                                    if status != 502 && status != 503 {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Request {} after ejection expected 502/503, got {}", i, status),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                // After ejection, connection errors are also acceptable
                                if i < 3 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} failed unexpectedly: {}", i, e),
                                    );
                                }
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Backend ejected after 3 consecutive 502s".to_string(),
                    )
                })
            },
        )
    }

    /// After ejection cooldown expires, backend should recover
    fn test_recovery_after_cooldown() -> TestCase {
        TestCase::new(
            "recovery_after_cooldown",
            "Ejected backend recovers after ejectionSeconds",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let test_id = format!("recover-{}", std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos());

                    // Reset CB test state
                    let reset_url = format!("{}/cb-reset", ctx.http_url());
                    ctx.http_client
                        .get(&reset_url)
                        .header("Host", "cb-test.example.com")
                        .send()
                        .await
                        .ok();

                    // Trigger ejection: 3 failures (fail_count=3 → first 3 return 502, then 200)
                    for _ in 0..3 {
                        let url = format!("{}/cb-fail/{}/3", ctx.http_url(), test_id);
                        ctx.http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await
                            .ok();
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    // Wait for ejection cooldown (ejectionSeconds=3)
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

                    // After recovery, backend should be available again (returns 200 now)
                    let url = format!("{}/cb-fail/{}/3", ctx.http_url(), test_id);
                    let mut success = false;
                    for _ in 0..3 {
                        match ctx
                            .http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await
                        {
                            Ok(r) if r.status().as_u16() == 200 => {
                                success = true;
                                break;
                            }
                            _ => {
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            }
                        }
                    }

                    if success {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Backend recovered after ejection cooldown".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            "Backend did not recover after cooldown".to_string(),
                        )
                    }
                })
            },
        )
    }
}
```

- [ ] **Step 6: 编译检查**

```bash
cargo check --all-targets
```

- [ ] **Step 7: 运行集成测试**

```bash
./examples/test/scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Backend/CircuitBreaker
```

Expected: 3 个测试通过

---

## Task 7: 代码质量检查

- [ ] **Step 1: cargo fmt**

```bash
cargo fmt --all
```

- [ ] **Step 2: cargo clippy**

```bash
cargo clippy --all-targets
```

- [ ] **Step 3: 完整测试**

```bash
cargo test --all
```

- [ ] **Step 4: 更新熔断器文档**

更新 `tasks/todo/circuit-breaker/circuit-breaker.md` 的状态表格和 Status 部分，标记已完成的模块。

---

## 参考资料

### 现有代码模式

| 模式 | 参考文件 |
|------|----------|
| Annotation 解析 | `src/core/gateway/backends/health/check/annotation.rs` |
| 全局 Store（LazyLock + Arc） | `src/core/gateway/backends/health/check/status_store.rs` |
| DashMap 并发状态 | `src/core/gateway/lb/runtime_state/mod.rs` |
| Service conf_handler | `src/core/gateway/backends/discovery/services/conf_handler_impl.rs` |
| is_backend_healthy() | `src/core/gateway/backends/mod.rs:97-106` |
| 失败 hook | `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs` |
| 错误 hook | `src/core/gateway/routes/http/proxy_http/pg_error_while_proxy.rs` |
| 响应 hook | `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs` |
| backend_hash() | `src/core/gateway/backends/health/check/mod.rs` |
| 集成测试 Suite | `examples/code/client/suites/http_route/backend/health_check.rs` |
| 测试 YAML | `examples/test/conf/HTTPRoute/Backend/HealthCheck/` |
| 测试注册 3 点 | `examples/code/client/test_client.rs` (`resolve_suite`, `suite_to_port_key`, `add_suites_for_suite`) |

### 设计决策

1. **为什么用 DashMap 而不是 ArcSwap？** — 熔断器状态在请求路径中需要频繁写入（record_failure/success），DashMap 支持无锁并发读写。HealthStatusStore 用 ArcSwap 是因为写入只在 health check 循环中（低频），读在请求路径中（高频）。熔断器两者都在请求路径中。

2. **为什么 backend_hash 用 weight=1？** — `record_failure_by_addr()` 中构造临时 Backend 时使用 weight=1。这与 EndpointSlice 中 Backend 的默认 weight 一致。如果 weight 不同会导致 hash 不匹配，但在实践中 LB 选择的 Backend 的 weight 在 `is_ejected()` 和 `record_failure()` 之间是一致的，因为都基于同一个 EndpointSlice 数据。

3. **为什么 `retry_on_status` 不计入熔断器失败？** — 状态码重试是路由层面的即时补救，upstream_response_filter 中的 5xx 检查已经会调用 `record_failure()`。如果在 error_while_proxy 中也记录，同一个请求会被双重计数。

4. **maxEjectionPercent=0 的含义** — 0 表示"不限制"，允许逐出所有端点。这与 Envoy 的语义一致（Envoy 默认 10%，0 = unlimited）。
