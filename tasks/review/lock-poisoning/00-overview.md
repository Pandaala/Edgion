# 锁中毒 (Lock Poisoning) 全局 Review

## 背景

`std::sync::RwLock` 和 `std::sync::Mutex` 在持锁线程 panic 时会标记锁为 "poisoned"。后续任何 `.unwrap()` / `.expect()` 获取锁的操作都会 panic，形成级联故障。

本 Review 全面扫描 Edgion 项目中所有 `std::sync` 锁的使用，逐模块评估风险并给出最佳处理方案。

---

## 全局统计

| 指标 | 数值 |
|------|------|
| 受影响文件数 | **48** |
| `.unwrap()` / `.expect()` 获取锁的调用点 | **298** |
| `unwrap_or_else` poison 恢复点 | **12** |
| 已使用 `parking_lot` 的文件 | **2**（`cache/lru.rs`、`routes/udp/edgion_udp.rs`） |
| `parking_lot = "0.12"` 依赖 | 已声明（`Cargo.toml:130`），几乎未使用 |

### 按二进制分布

| 二进制 | 锁调用点 | 占比 |
|--------|---------|------|
| Gateway（数据面） | 128 | 41% |
| Controller（控制面） | 152 | 49% |
| Center（联邦中心） | 18 | 6% |
| Common（公共） | ~12（声明在 common，调用在其他模块） | 4% |

### 按处理模式分布

| 模式 | 数量 | 文件 |
|------|------|------|
| `.unwrap()` | ~280 | 绝大多数文件 |
| `.expect("...")` | ~18 | `tls_store.rs`、`health/check/`、`udp/mod.rs` |
| `.unwrap_or_else(\|e\| e.into_inner())` | 12 | `tls_store.rs`(2)、`handler.rs`(2)、`base.rs`(1)、`acme/service.rs`(5)、`backends/mod.rs`(1)、`cache_client event_dispatch`(1) |

### 锁密度最高的文件（Top 10）

| 文件 | 锁调用点数 | 锁类型 |
|------|-----------|--------|
| `controller/conf_sync/cache_server/cache.rs` | 19 | RwLock |
| `gateway/backends/discovery/endpoint_slice/ep_slice_store.rs` | 18 | RwLock + Mutex |
| `controller/conf_sync/conf_server/config_sync_server.rs` | 15 | RwLock |
| `controller/resource_processor/gateway_route_index.rs` | 15 | RwLock ×4 |
| `controller/resource_processor/ref_grant/store.rs` | 14 | RwLock ×2 |
| `gateway/config/gateway_class/conf_handler_impl.rs` | 13 | RwLock (global static) |
| `gateway/config/edgion_gateway/conf_handler_impl.rs` | 13 | RwLock (global static) |
| `gateway/conf_sync/cache_client/cache.rs` | 12 | RwLock |
| `gateway/backends/discovery/endpoint/endpoint_store.rs` | 12 | RwLock + Mutex |
| `controller/resource_processor/listener_port_manager.rs` | 10 | RwLock ×2 |

---

## 核心问题（经深入分析修正）

### 1. 处理策略四种模式并存，缺乏统一规范

| 模式 | 文件 | 评价 |
|------|------|------|
| 裸 `.unwrap()` | ~280 处（绝大多数） | 一致地 panic，至少策略统一 |
| `.expect("...")` | `tls_store.rs` 读端、`health/check/`、`udp/mod.rs` | 带上下文的 panic，比 unwrap 略好 |
| `.unwrap_or_else(\|e\| e.into_inner())` | `tls_store.rs` 写端、`handler.rs`、`base.rs`、`backends/mod.rs` | 恢复 poison，但如果读端不配合则形同虚设 |
| `map_err` 转业务错误 | `jwt_auth/plugin.rs` | **最佳实践** — 不 panic，优雅降级 |

**"半恢复"比"不恢复"更危险**：`tls_store.rs` 写端恢复了数据但 8 个读端会 panic，恢复完全无效。

### 2. P0 风险：`creation_lock` Mutex 冻结后端发现

`ep_slice_store.rs` / `endpoint_store.rs` 的 `creation_lock` Mutex 用于 DCL（双检锁）模式延迟创建 LB 实例。这是全局 `LazyLock` 单例，如果 creation_lock poison：
- 所有后续 LB 创建/更新操作永久阻塞
- 最终所有流量黑洞
- 进程内不可恢复

### 3. P1 风险：ACME Challenge Store 在每请求热路径

`challenge_store.rs` 的 `is_empty()` 和 `lookup()` 在每个 HTTP 请求的 `early_request_filter` 中调用。poison 导致**所有 HTTP 流量** panic。

### 4. 全局单例不可替换

多个关键 store 使用 `LazyLock<Arc<...>>` 或 `static` 单例，一旦 poison 进程内无法恢复：
- `GLOBAL_TLS_STORE`、`EDGION_GATEWAY_CONFIG_STORE`、`GATEWAY_CLASS_STORE`
- `GLOBAL_EP_SLICE_STORE`、`GLOBAL_ENDPOINT_STORE`（含 creation_lock）
- `ACME_SERVICE`

### 5. 多锁同时获取 — poison 级联风险

以下模块在同一方法内获取 2+ 把锁（顺序一致，无死锁风险，但有 poison 级联风险）：
- `ep_slice_store.rs`：`creation_lock` → `service_to_slices` + `ep_slices`（最危险，P0）
- `gateway_route_index.rs`：`forward` + `reverse` + `gateway_hostnames` + `gateway_ports`（最多 4 把）
- `listener_port_manager.rs`：`gateway_to_ports` + `port_to_listeners`
- `ref_grant/store.rs`：`grants` + `grants_by_to_namespace`

### 6. 正面发现：两个模块已正确处理

- **`controller/services/acme/service.rs`**：所有 11 处读写均使用 `unwrap_or_else` 恢复，策略完全一致。被保护的数据是临时状态（真实来源是 K8s CRD），恢复后不影响正确性。
- **`gateway/plugins/http/jwt_auth/plugin.rs`**：使用 `map_err` 将锁错误转为认证失败，不 panic，是项目中的最佳实践。

---

## 推荐方案：全局迁移至 `parking_lot`

### 为什么选 `parking_lot`

| 维度 | `std::sync` + poison 恢复 | `parking_lot` |
|------|--------------------------|---------------|
| poison 问题 | 需要每个调用点显式处理 | **不存在**（设计上消除） |
| API 兼容性 | — | 几乎 1:1 替换 |
| 性能 | 标准 | 通常更优（fast-path spinning） |
| 依赖 | — | 已声明 `Cargo.toml:130` |
| 迁移成本 | 每处加恢复逻辑 ×298 | 改 import + 删 unwrap ×298 |
| 代码简洁度 | 大量 boilerplate | `.read()` / `.write()` 直接返回 guard |

### 迁移模式

```rust
// Before (std::sync::RwLock)
use std::sync::RwLock;
let data = self.store.read().unwrap();
let mut data = self.store.write().unwrap();

// After (parking_lot::RwLock)
use parking_lot::RwLock;
let data = self.store.read();
let mut data = self.store.write();
```

```rust
// Before (std::sync::Mutex)
use std::sync::Mutex;
let guard = self.inner.lock().unwrap();

// After (parking_lot::Mutex)
use parking_lot::Mutex;
let guard = self.inner.lock();
```

### 迁移优先级（经 subagent 深入分析后修正）

| 优先级 | 模块 | 理由 | 详细报告 |
|--------|------|------|---------|
| **P0** | Gateway Backends — `creation_lock` | DCL 模式的 Mutex，poison 后整个后端发现子系统永久冻结，全局单例不可重建，所有流量黑洞 | [03](03-gateway-backends.md) |
| **P1** | Gateway Routes — ACME Challenge Store | 每个 HTTP 请求热路径调用 `is_empty()` + `lookup()`，poison 导致**所有 HTTP 流量** panic | [05](05-gateway-routes-services.md) |
| **P1** | Gateway Backends — 数据层 RwLock | endpoint/ep_slice 存储，在 `creation_lock` 范围内访问，poison 级联 | [03](03-gateway-backends.md) |
| **P1** | Controller ConfSync — `cache_server/cache.rs` | 锁密度最高(19处)，watcher 热循环中 `store.read().unwrap()`，poison 切断所有 Gateway watch 流 | [08](08-controller-confsync-center.md) |
| **P1** | Controller ConfSync — `config_sync_server.rs` | `watch_objects` RwLock 是单点故障，poison 导致所有资源类型的 list/watch 同时失败 | [08](08-controller-confsync-center.md) |
| **P2** | Gateway TLS Store | 读写 poison 策略矛盾，但 TLS 握手热路径实际用 ArcSwap 而非 RwLock，仅影响诊断/stats API | [01](01-gateway-tls-store.md) |
| **P2** | Gateway Cache & Config | 全局 static store 不可替换，但不在请求热路径上（config sync only） | [04](04-gateway-cache-config.md) |
| **P2** | Controller Conf Center | ProcessorRegistry 在半热路径上，health check / shutdown 路径有风险 | [06](06-controller-conf-center.md) |
| **P2** | Controller Resource Processors | 13 个文件 ~81 处，多锁同时获取但顺序一致，控制面非热路径 | [07](07-controller-resource-processors.md) |
| **P2** | Gateway Backends — Health Check | poison 导致不健康后端继续接收流量（默认健康） | [03](03-gateway-backends.md) |
| **P3** | Gateway Runtime | 仅 3 个写入点且均已恢复，无生产读调用。`partial_update` 临界区过宽是额外问题 | [02](02-gateway-runtime.md) |
| **P3** | Center 模块 | 独立二进制，Controller 自动重连，影响范围有限 | [08](08-controller-confsync-center.md) |
| **N/A** | Controller ACME Service | **已正确处理** — 所有 11 处读写均使用 `unwrap_or_else` 恢复，策略一致 | [08](08-controller-confsync-center.md) |
| **N/A** | Gateway JWT Auth Plugin | **已正确处理** — 使用 `map_err` 将锁错误转为认证失败，项目中最佳实践 | [05](05-gateway-routes-services.md) |

---

## 各模块详细报告

| # | 报告 | 文件数 | 锁调用点 | 关键风险 | 深入分析后优先级 |
|---|------|--------|---------|---------|----------------|
| 01 | [Gateway TLS Store](01-gateway-tls-store.md) | 1 | 10 | 读写 poison 策略矛盾（但热路径用 ArcSwap） | P2 |
| 02 | [Gateway Runtime](02-gateway-runtime.md) | 2 | 3 | 全局 store 写入点（已有恢复，无读调用） | P3 |
| 03 | [Gateway Backends](03-gateway-backends.md) | 8 | 55 | **creation_lock poison 冻结后端发现**，多锁级联 | **P0** |
| 04 | [Gateway Cache & Config](04-gateway-cache-config.md) | 6 | 33 | 全局 static store 不可替换 | P2 |
| 05 | [Gateway Routes & Services](05-gateway-routes-services.md) | 6 | 21 | **ACME 每请求热路径**，JWT 已正确处理 | **P1** |
| 06 | [Controller Conf Center](06-controller-conf-center.md) | 3 | 25 | shutdown 路径 + ProcessorRegistry 半热路径 | P2 |
| 07 | [Controller Resource Processors](07-controller-resource-processors.md) | 13 | ~81 | 最多文件，多锁同时获取 | P2 |
| 08 | [Controller ConfSync & Center](08-controller-confsync-center.md) | 9 | 72 | **cache.rs watcher 循环**、ACME 已正确处理 | **P1** |

---

## 注意事项

1. **`parking_lot::RwLock` 不支持 `into_inner()` 从 `PoisonError` 恢复** — 因为不存在 poison，迁移后所有 `unwrap_or_else` 恢复逻辑可以直接删除
2. **`parking_lot::RwLock` 的 guard 不是 `Send`** — 如果有跨 `.await` 持锁的场景需要检查（但 `std::sync` 的 guard 也不是 `Send`，所以不会有新问题）
3. **测试代码中的 `.unwrap()` 不需要迁移** — 测试中 panic 是正常行为
4. **两个文件已使用 `parking_lot`** — `cache/lru.rs` 和 `routes/udp/edgion_udp.rs`，证明迁移路径已验证

---

## 额外发现（subagent 深入分析产出）

| 发现 | 模块 | 报告 |
|------|------|------|
| TLS 握手热路径实际用 `ArcSwap`，RwLock 仅影响诊断 API | TLS Store | [01](01-gateway-tls-store.md) |
| `partial_update` 临界区过宽，应与 `full_set` 对齐（先释放锁再 rebuild） | Runtime | [02](02-gateway-runtime.md) |
| `BackendTLSPolicyStore` 读路径已有部分防御 (`map().unwrap_or()`) | Backends | [03](03-gateway-backends.md) |
| `ConfigClient.base_conf: RwLock<Option<GatewayBaseConf>>` 零引用，疑似死代码 | Cache/Config | [04](04-gateway-cache-config.md) |
| `RouteRules` Clone impl 中的 RwLock 字段疑似运行时未使用 | Routes | [05](05-gateway-routes-services.md) |
| `ref_manager.rs` 已有双锁→单锁重构先例，团队有多锁意识 | Resource Processors | [07](07-controller-resource-processors.md) |
| ACME service 是唯一完整 poison 恢复模块，可作为参考但 parking_lot 更优 | ConfSync/Center | [08](08-controller-confsync-center.md) |

---

*Review 日期: 2026-04-09*
