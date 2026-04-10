# Controller Resource Processors — 锁中毒 Review

## 模块概述

`resource_processor/` 是 Controller 侧的核心状态管理层，负责 K8s 资源的存储、索引和跨资源引用追踪。这些模块在配置同步周期（init LIST + runtime watch events）中被调用，不在每请求热路径上。但它们构成配置管道的基础设施——任一锁中毒都会导致后续所有配置更新无法处理，Controller 功能性瘫痪。

所有文件统一使用 `std::sync::RwLock` + `.unwrap()` 模式，无任何错误恢复处理。

## 锁使用清单

| 文件 | RwLock 字段数 | `.read()/.write().unwrap()` 调用数 | 多锁同时持有 |
|------|-------------|----------------------------------|------------|
| `gateway_route_index.rs` | 4 (`forward`, `reverse`, `gateway_hostnames`, `gateway_ports`) | 15 | **是**: `forward` + `reverse` 同时 write (3 处) |
| `listener_port_manager.rs` | 2 (`port_to_listeners`, `gateway_to_ports`) | 10 | **是**: `gateway_to_ports` + `port_to_listeners` 同时持有 (5 处) |
| `ref_grant/store.rs` | 2 (`grants`, `grants_by_to_namespace`) | 12 | **是**: `grants` read + `grants_by_to_namespace` write (2 处) |
| `attached_route_tracker.rs` | 2 (`forward`, `reverse`) | 6 | **是**: `forward` + `reverse` 同时 write (2 处) |
| `ref_manager.rs` | 1 (`inner: RwLock<RefMaps>`) | 8 | 否 (已合并为单锁) |
| `processor.rs` | 3 (`raw_store`, `metadata_filter`, `namespace_filter`) | 8 | 否 (均独立访问) |
| `backend_tls_policy_ref.rs` | 1 (`timestamps`) + 内嵌 `target_mgr` | 3 (timestamps 自身) | 否 (timestamps 与 target_mgr 不同时持有) |
| `namespace_store.rs` | 1 (`store`) | 4 | 否 |
| `ref_grant/events.rs` | 1 (`listeners`) | 3 | 否 |
| `configmap_utils/configmap_store.rs` | 1 (`configmaps`) | 4 | 否 |
| `secret_utils/secret_store.rs` | 1 (`secrets`) | 3 | 否 |
| `handlers/secret.rs` | 1 (`init_accumulator`) | 2 | 否 |
| `raw_store.rs` (FsRawStore) | 1 (`data`) | 3 | 否 |
| **合计** | **21** | **~81** | **4 个文件存在多锁同时持有** |

## 问题分析

### 1. 多锁同时获取模式（核心风险）

四个文件存在同时持有两个锁的模式：

**`gateway_route_index.rs`** — `update_route()` / `remove_route()` 同时 write `forward` + `reverse`：
```rust
let mut fwd = self.forward.write().unwrap();   // 锁1
let mut rev = self.reverse.write().unwrap();   // 锁2
```
`clear()` 方法则逐个获取 4 个锁（非同时），存在窗口不一致但无死锁风险。

**`listener_port_manager.rs`** — 所有 4 个公开方法都同时持有两个锁，且有明确的锁顺序注释：
```rust
// # Lock Order
// Always acquires locks in order: gateway_to_ports -> port_to_listeners
let mut gw_map = self.gateway_to_ports.write().unwrap();
let mut port_map = self.port_to_listeners.write().unwrap();
```
锁顺序一致（所有 5 处均为 `gateway_to_ports` -> `port_to_listeners`），死锁已规避。但中毒级联风险存在：若 `gateway_to_ports` 在持锁期间 panic，`port_to_listeners` 不受影响；但若 `port_to_listeners` 在持锁期间 panic，`gateway_to_ports` 也同时被标记为中毒（因为 MutexGuard drop 时 panic 不会自动 poison 其他锁，但 panic 展开会导致两个 guard 都 drop 而触发写锁的 poison）。

**`ref_grant/store.rs`** — `rebuild_namespace_index()` 和 `rebuild_indexes_incremental()` 同时持有 `grants.read()` + `grants_by_to_namespace.write()`：
```rust
let grants = self.grants.read().unwrap();
let mut index = self.grants_by_to_namespace.write().unwrap();
```
`replace_all()` 则分步获取两个写锁（先 `grants` 后 `grants_by_to_namespace`），与 `rebuild_*` 方法的获取顺序一致，无死锁。

**`attached_route_tracker.rs`** — `update_route()` / `remove_route()` 同时 write `forward` + `reverse`，模式与 `gateway_route_index` 相同。

### 2. 级联中毒导致 Controller 不可恢复

所有锁都保护 Controller 配置状态。中毒传播路径：

1. 任一 handler `on_change`/`parse` 中的 panic（例如意外的 K8s 资源结构）触发 `write().unwrap()` 时锁被 poison
2. 该锁后续所有 `read().unwrap()` / `write().unwrap()` 都会 panic
3. 对于多锁同时持有的场景，一个锁 poison 会在后续获取组合锁时 panic，间接 poison 第二个锁
4. 最终所有依赖该模块的配置处理停止工作

典型场景：`GatewayRouteIndex::update_route()` 中 `forward.write()` 成功但在操作过程中 panic -> `forward` 被 poison -> 后续 `get_routes_for_gateway()` 调用 `forward.read().unwrap()` 也 panic -> 所有 Gateway 变更触发的路由重排失败。

### 3. `ref_manager.rs` 的正面案例

`ref_manager.rs` 已经完成了从双锁到单锁的重构，文档注释明确记录了原因：

> An earlier design used two separate locks — one per index — which introduced an AB-BA deadlock: `add_ref` acquired `refs` then `dependencies`, while `clear_value_refs` acquired them in the opposite order. The single-lock design eliminates that hazard.

这说明项目已经意识到多锁风险并做过修复，但 `gateway_route_index.rs`、`listener_port_manager.rs`、`attached_route_tracker.rs` 仍保留多锁设计（虽然锁顺序一致，不会死锁，但 poison 级联风险仍在）。

### 4. 一致的 unwrap() 模式：利大于弊

所有 13 个文件统一使用 `.unwrap()`，无混合处理。这实际上是合理的：
- 这些锁保护的是 Controller 核心配置状态，poison 后数据一致性已不可保证
- 混合 `unwrap()` + `unwrap_or_else(recover)` 反而可能掩盖严重一致性问题
- 真正的修复不是更好地处理 poison，而是彻底消除 poison 可能性

## 最佳处理方案

### 推荐：批量迁移到 `parking_lot::RwLock`

**理由**：
1. `parking_lot` 已在 `Cargo.toml` 中（`parking_lot = "0.12"`），零引入成本
2. `parking_lot::RwLock` 不会 poison——panic 时锁自动释放，后续调用者不受影响
3. API 兼容——仅需替换 `use std::sync::RwLock` 为 `use parking_lot::RwLock`，删除所有 `.unwrap()` 调用（`parking_lot` 的 `read()`/`write()` 直接返回 guard，不是 `Result`）
4. 副收益：`parking_lot::RwLock` 不需要指定 `unwrap()`，代码更简洁；且在无竞争场景下性能略优

**迁移步骤**：
1. 逐文件替换 `use std::sync::RwLock` -> `use parking_lot::RwLock`
2. 删除所有 `.read().unwrap()` -> `.read()`、`.write().unwrap()` -> `.write()`
3. 注意：`parking_lot::RwLock` 不支持 `RwLock::into_inner()` 和 `try_read()`/`try_write()` 的返回类型不同，需检查是否有这些用法（本批文件未使用）
4. 每个文件独立提交，便于 review

**对多锁同时持有文件的额外建议**：
- `gateway_route_index.rs` 和 `attached_route_tracker.rs`：考虑参照 `ref_manager.rs` 合并为单锁 `RwLock<IndexMaps>`，消除 `clear()` 的窗口不一致
- `listener_port_manager.rs`：已有严格锁顺序注释，迁移 parking_lot 后无 poison 风险，可保持双锁设计
- `ref_grant/store.rs`：`rebuild_namespace_index()` 同时持有 grants read + index write，迁移 parking_lot 后可接受；若后续重构，可考虑合并为单锁

## 优先级

**P2 — 建议近期批量处理**

理由：
- 影响面：13 个文件、~81 个 unwrap 调用点，覆盖 Controller 全部核心状态管理
- 风险等级：中。控制面代码，非热路径，panic 概率低。但一旦发生，Controller 完全失去配置更新能力，需要重启恢复
- 修复成本：低。纯机械替换，parking_lot 已是依赖项，API 近乎兼容
- 收益：消除整个 Controller 配置管道的 poison 级联风险；代码更简洁（删除所有 `.unwrap()`）
- 可与 Gateway 侧的 parking_lot 迁移合并为一个统一 PR
