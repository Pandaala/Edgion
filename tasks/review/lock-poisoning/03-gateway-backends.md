# Gateway Backends — 锁中毒 Review

## 模块概述

Gateway Backends 是数据面的核心路径，负责后端发现（EndpointSlice / Endpoint）、健康检查和 TLS 策略管理。所有 store 均为 `static LazyLock<Arc<…>>` 全局单例，一旦中毒则整个 Gateway 生命周期内无法恢复。

全部使用 `std::sync::{RwLock, Mutex}`，项目虽依赖 `parking_lot = "0.12"` 但此模块未使用（parking_lot 的锁不会中毒）。

## 锁使用清单

### EndpointSlice Discovery (`discovery/endpoint_slice/`)

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 1 | `ep_slice_store.rs` | 136 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 2 | `ep_slice_store.rs` | 168 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 3 | `ep_slice_store.rs` | 174 | `service_to_slices.read().unwrap()` | RwLock | unwrap | 高 |
| 4 | `ep_slice_store.rs` | 175 | `ep_slices.read().unwrap()` | RwLock | unwrap | 高 |
| 5 | `ep_slice_store.rs` | 196 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 6 | `ep_slice_store.rs` | 210 | `service_to_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 7 | `ep_slice_store.rs` | 211 | `ep_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 8 | `ep_slice_store.rs` | 227 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 9 | `ep_slice_store.rs` | 230 | `service_to_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 10 | `ep_slice_store.rs` | 231 | `ep_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 11 | `ep_slice_store.rs` | 301 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 12 | `ep_slice_store.rs` | 389 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 13 | `ep_slice_store.rs` | 451 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 14 | `ep_slice_store.rs` | 515 | `service_to_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 15 | `ep_slice_store.rs` | 516 | `ep_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 16 | `ep_slice_store.rs` | 535 | `ep_slices.read().unwrap()` | RwLock | unwrap | 高 |
| 17 | `ep_slice_store.rs` | 559 | `ep_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 18 | `ep_slice_store.rs` | 560 | `service_to_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 19 | `discovery_impl.rs` | 118 | `endpoint_slices.write().unwrap()` | RwLock | unwrap | 高 |
| 20 | `discovery_impl.rs` | 139 | `endpoint_slices.read().unwrap()` | RwLock | unwrap | 中 |
| 21 | `discovery_impl.rs` | 153 | `endpoint_slices.read().unwrap()` | RwLock | unwrap | 中 |
| 22 | `discovery_impl.rs` | 280 | `discovery.endpoint_slices.read().unwrap()` | RwLock | unwrap | 中 |

### Endpoint Discovery (`discovery/endpoint/`)

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 23 | `endpoint_store.rs` | 117 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 24 | `endpoint_store.rs` | 125 | `ep_data.read().unwrap()` | RwLock | unwrap | 高 |
| 25 | `endpoint_store.rs` | 148 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 26 | `endpoint_store.rs` | 149 | `ep_data.read().unwrap()` | RwLock | unwrap | 高 |
| 27 | `endpoint_store.rs` | 167 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 28 | `endpoint_store.rs` | 198 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 29 | `endpoint_store.rs` | 201 | `ep_data.write().unwrap()` | RwLock | unwrap | 高 |
| 30 | `endpoint_store.rs` | 216 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 31 | `endpoint_store.rs` | 219 | `ep_data.write().unwrap()` | RwLock | unwrap | 高 |
| 32 | `endpoint_store.rs` | 245 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 33 | `endpoint_store.rs` | 255 | `ep_data.read().unwrap()` | RwLock | unwrap | 高 |
| 34 | `endpoint_store.rs` | 340 | `creation_lock.lock().unwrap()` | Mutex | unwrap | **严重** |
| 35 | `discovery_impl.rs` | 122 | `endpoint.read().unwrap()` | RwLock | unwrap | 中 |
| 36 | `discovery_impl.rs` | 136 | `endpoint.write().unwrap()` | RwLock | unwrap | 高 |
| 37 | `discovery_impl.rs` | 147 | `endpoint.read().unwrap()` | RwLock | unwrap | 中 |
| 38 | `discovery_impl.rs` | 153 | `endpoint.read().unwrap()` | RwLock | unwrap | 中 |
| 39 | `discovery_impl.rs` | 164 | `endpoint.read().unwrap()` | RwLock | unwrap | 中 |
| 40 | `discovery_impl.rs` | 269 | `discovery.endpoint.read().unwrap()` | RwLock | unwrap | 中 |

### Health Check (`health/check/`)

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 41 | `manager.rs` | 92 | `tasks.write().expect("lock tasks")` | RwLock | expect | 高 |
| 42 | `manager.rs` | 100 | `tasks.write().expect("lock tasks")` | RwLock | expect | 高 |
| 43 | `status_store.rs` | 109 | `states.lock().expect("lock service states")` | Mutex | expect | 高 |
| 44 | `status_store.rs` | 153 | `states.lock().expect("lock service states")` | Mutex | expect | 高 |
| 45 | `status_store.rs` | 169 | `services_update_lock.lock().expect("lock services update")` | Mutex | expect | 高 |
| 46 | `status_store.rs` | 185 | `services_update_lock.lock().expect("lock services update")` | Mutex | expect | 高 |

### BackendTLS Policy (`policy/backend_tls/`)

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 47 | `backend_tls_policy_store.rs` | 147-148 | `policies.read()` → `.map().unwrap_or(false)` | RwLock | 降级处理 | **低** |
| 48 | `backend_tls_policy_store.rs` | 154-155 | `policies.read()` → `.ok().and_then()` | RwLock | 降级处理 | **低** |
| 49 | `backend_tls_policy_store.rs` | 170 | `policies.write().unwrap()` | RwLock | unwrap | 高 |
| 50 | `backend_tls_policy_store.rs` | 192 | `policies.write().unwrap()` | RwLock | unwrap | 高 |
| 51 | `backend_tls_policy_store.rs` | 290-291 | `policies.read()` → `.map().unwrap_or_default()` | RwLock | 降级处理 | **低** |
| 52 | `backend_tls_policy_store.rs` | 298-301 | `policies.read()` → `.map().unwrap_or_default()` | RwLock | 降级处理 | **低** |
| 53 | `backend_tls_policy_store.rs` | 328 | `policies.read().map().unwrap_or(0)` | RwLock | 降级处理 | **低** |
| 54 | `backend_tls_policy_store.rs` | 333 | `policies.read().map().unwrap_or(0)` | RwLock | 降级处理 | **低** |

### ConsistentHash Ring Lock (`mod.rs`)

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 55 | `mod.rs` | 509 | `ring_lock.read().unwrap_or_else(\|e\| e.into_inner())` | RwLock | 中毒恢复 | **低** |

**锁获取点总计：55 处**

## 问题分析

### 1. `creation_lock` Mutex — 最高风险（12处）

`creation_lock` 是 DCL（Double-Checked Locking）模式的关键序列化锁。`EpSliceStore` 和 `EndpointStore` 各持有一个，保护"数据层读取 → LB 创建/更新"的原子性。

**中毒场景**：如果任何持有 `creation_lock` 的代码路径 panic（例如 `EndpointSliceLoadBalancer::new_with_slices()` 内部构建失败），则 Mutex 永久中毒。

**连锁影响**：
- `get_or_create()` — 新 Service 的 LB 永远无法创建，请求路由到该 Service 时返回 502
- `replace_data_only()` / `update_data_only()` — 所有后续 K8s Endpoint 同步被阻塞，整个数据层冻结
- `update_lb_if_exists()` — 已有 LB 无法更新后端列表，滚动升级时后端列表过期
- `get_slices_for_service()` / `get_endpoint_for_service()` — 其他算法 store 无法获取数据

**严重度**：creation_lock 中毒 = **整个后端发现子系统瘫痪**，所有流量最终黑洞。这是全局单例（`LazyLock`），没有重建路径。

### 2. 数据层 RwLock（`service_to_slices`、`ep_slices`、`ep_data`）— 高风险（18处）

这些 RwLock 保护 Endpoint 数据的内存存储。虽然大部分只在 `creation_lock` 内部访问（间接受保护），但：
- `discovery_impl.rs` 中的 `endpoint_slices` RwLock 是独立于 `creation_lock` 的，由 `MultiEndpointSliceDiscovery` 和 `EndpointDiscovery` 内部持有
- deprecated 方法 `update_with_service_aggregation()` 直接在 `creation_lock` 外部访问 `ep_slices.read()` (L535)

**中毒场景**：数据层 write 路径 panic（e.g., HashMap 分配 OOM），后续所有 read/write 全部 panic。

### 3. 健康检查 Store — 高风险（6处）

`HealthCheckManager::tasks` 使用 `RwLock`，`HealthStatusStore::states` 使用 `Mutex`。

**中毒场景**：
- `tasks.write().expect("lock tasks")` — 如果 `tokio::runtime::Handle::spawn` 在持锁状态 panic，后续所有健康检查任务的增删操作失败
- `states.lock().expect("lock service states")` — 记录探测结果时 panic，该 Service 的所有后续健康状态更新丢失

**影响**：健康检查中毒不会直接阻塞流量，但会导致不健康的后端持续接收流量（默认 healthy），间接导致请求失败率上升。

### 4. BackendTLS Policy Store — 部分已有降级处理

**亮点**：`contains()`、`get()`、`get_all()`、`get_by_namespace()`、`count()`、`stats()` 的读路径已使用 `.read().map().unwrap_or()` 模式，中毒时返回空值/false 而非 panic。这是整个 backends 模块中唯一做了防御性处理的。

**问题**：`replace_all()` (L170) 和 `update()` (L192) 的 write 路径仍使用裸 `.write().unwrap()`。中毒后写入失败，但至少读路径能降级。

### 5. ConsistentHash Ring — 已正确处理

`mod.rs:509` 使用 `ring_lock.read().unwrap_or_else(|e| e.into_inner())`，中毒时通过 `into_inner()` 获取内部数据继续服务。这是正确的做法。

**不一致性**：这是整个 backends 模块 55 处锁获取中 **唯一** 使用中毒恢复的地方，其余全部 unwrap/expect。

### 6. `creation_lock` 与内部 RwLock 的嵌套持锁

`ep_slice_store.rs` 和 `endpoint_store.rs` 中的 DCL 模式如下：
```
creation_lock.lock()  →  ep_data.read()  →  drop(ep_data)  →  [create LB]  →  ArcSwap.store()
```
如果 `creation_lock` 中毒，内部的所有 RwLock 操作都不可达。但反过来，内部 RwLock 中毒时，外部 `creation_lock` 还未释放，因此 `creation_lock` 也会被标记为中毒（持有 `creation_lock` 的线程 panic 了）。一个内部 RwLock 的 panic 会导致 `creation_lock` **连带中毒**，形成级联故障。

## 最佳处理方案

### 方案 A：切换到 `parking_lot`（推荐）

项目已依赖 `parking_lot = "0.12"`，其 `Mutex`/`RwLock` **不实现中毒语义**，panic 时锁自动释放，后续获取不受影响。

变更范围：
```rust
// Before
use std::sync::{Mutex, RwLock};

// After
use parking_lot::{Mutex, RwLock};
```

注意 API 差异：
- `parking_lot::Mutex::lock()` 直接返回 `MutexGuard`（无 `Result`）
- `parking_lot::RwLock::read()` 直接返回 `RwLockReadGuard`（无 `Result`）
- 无需 `.unwrap()` / `.expect()`，代码更简洁

影响文件（4个 store + 2个 discovery_impl + manager + status_store）：8个文件，55处修改。

### 方案 B：统一 `unwrap_or_else` + `into_inner()`

对所有 `std::sync::RwLock` 的获取统一改为中毒恢复模式（与 `mod.rs:509` 一致）：
```rust
// RwLock
let guard = lock.read().unwrap_or_else(|e| e.into_inner());
let guard = lock.write().unwrap_or_else(|e| e.into_inner());

// Mutex
let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
```

缺点：`into_inner()` 恢复的数据可能处于不一致状态（前一个 writer 部分写入后 panic）。对于 `creation_lock`（空 `()`），这无风险。对于数据 RwLock（HashMap），存在不一致风险。

### 方案 C：混合方案

- **`creation_lock` (Mutex<()>)**：切换到 `parking_lot::Mutex` 或使用 `unwrap_or_else(|e| e.into_inner())`，因为 `()` 类型无数据一致性问题
- **数据层 RwLock**：切换到 `parking_lot::RwLock`，避免中毒后数据不一致
- **`BackendTLSPolicyStore`**：保持当前读路径降级模式，write 路径切换到 `parking_lot`

## 优先级

| 优先级 | 目标 | 锁位点数 | 理由 |
|--------|------|----------|------|
| **P0** | `ep_slice_store.rs` — `creation_lock` (6处) | 6 | 全局单例，中毒后整个 EndpointSlice 发现瘫痪，所有流量黑洞 |
| **P0** | `endpoint_store.rs` — `creation_lock` (6处) | 6 | 同上，Endpoint 发现路径 |
| **P1** | `ep_slice_store.rs` — 数据层 RwLock (8处) | 8 | 与 creation_lock 级联中毒，数据层冻结 |
| **P1** | `endpoint_store.rs` — 数据层 RwLock (4处) | 4 | 同上 |
| **P1** | `discovery_impl.rs` (ep_slice) — RwLock (4处) | 4 | Pingora ServiceDiscovery trait 回调路径，中毒后 LB 无法刷新 |
| **P1** | `discovery_impl.rs` (endpoint) — RwLock (6处) | 6 | 同上 |
| **P2** | `manager.rs` — tasks RwLock (2处) | 2 | 健康检查任务管理，中毒后无法启停健康检查 |
| **P2** | `status_store.rs` — states Mutex + update_lock (4处) | 4 | 健康状态更新，中毒后不健康后端持续收到流量 |
| **P3** | `backend_tls_policy_store.rs` — write 路径 (2处) | 2 | 读路径已有降级，write 中毒后策略冻结但不立即影响流量 |
| **完成** | `mod.rs:509` — ring_lock | 1 | 已使用 `unwrap_or_else` 中毒恢复 |

**建议**：统一切换到 `parking_lot`（方案 A），一次性消除全部 55 处中毒风险。改动机械、低风险、无语义变化。项目已有 `parking_lot = "0.12"` 依赖，无需新增外部依赖。
