# Gateway Runtime — 锁中毒 Review

## 模块概述

Gateway Runtime 模块负责 Gateway 资源的生命周期管理，包括：
- `handler.rs`：`ConfHandler<Gateway>` 实现，处理 `full_set` / `partial_update` 配置同步回调
- `server/base.rs`：`GatewayBase::configure_listeners`，启动阶段将 Gateway 注入 Pingora Server
- `store/gateway.rs`：`GatewayStore` 全局状态，用 `std::sync::RwLock` 保护
- `store/config.rs`：`GatewayConfigStore`，使用 `ArcSwap`（无锁中毒风险）
- `store/listener_gateway_info.rs`：`ListenerGatewayInfoStore`，使用 `ArcSwap`（无锁中毒风险）
- `matching/tls.rs`：`GatewayTlsMatcher`，使用 `ArcSwap`（无锁中毒风险）

**唯一涉及 `std::sync::RwLock` 的全局状态是 `GLOBAL_GATEWAY_STORE`**（`Arc<RwLock<GatewayStore>>`）。

## 锁使用清单

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 1 | `runtime/handler.rs` | 41 | `write()` | `std::sync::RwLock` | `unwrap_or_else(\|e\| e.into_inner())` | 低 |
| 2 | `runtime/handler.rs` | 93 | `write()` | `std::sync::RwLock` | `unwrap_or_else(\|e\| e.into_inner())` | 中 |
| 3 | `runtime/server/base.rs` | 78 | `write()` | `std::sync::RwLock` | `unwrap_or_else(\|e\| e.into_inner())` | 低 |
| 4 | `routes/http/tests.rs` | 263 等 (29 处) | `write()` | `std::sync::RwLock` | `unwrap()` | 无（测试代码） |

**注意**：整个生产代码中，`GLOBAL_GATEWAY_STORE` **没有任何 `.read()` 调用**。所有读取路径通过 `store.list_gateways()` 在持有写锁期间完成，或经由 `ArcSwap` 的 `GatewayConfigStore` / `ListenerGatewayInfoStore` 间接访问快照数据。

## 问题分析

### 1. 写路径恢复模式正确性 -- 正确

三个生产 `write()` 调用均使用 `unwrap_or_else(|e| e.into_inner())` 恢复模式。对于 `GatewayStore` 的数据结构（`HashMap<String, Gateway>`），这是安全的：

- `GatewayStore` 内部方法（`clear`、`add_gateway`、`update_gateway`、`remove_gateway`、`list_gateways`）全部是简单的 HashMap CRUD 操作
- 这些方法中**没有 `unwrap()`、`expect()`、`panic!()`**——不存在自身引发 panic 的风险
- 数据类型 `Gateway` 是纯 Serde 结构体，`clone()` 不会 panic

### 2. 缺失 read 路径处理 -- 不适用

`GLOBAL_GATEWAY_STORE` 在生产代码中没有 `.read()` 调用。所有消费者通过以下方式访问数据：
- **写锁内部读取**：`store.list_gateways()` 在 `full_set` 写锁持有期间调用（handler.rs:62, 179）
- **ArcSwap 快照**：`GatewayConfigStore`、`ListenerGatewayInfoStore` 在写锁释放后通过 `ArcSwap::store()` 发布不可变快照，读取路径完全无锁

因此不存在 "read 路径未处理中毒" 的问题。

### 3. `partial_update` 临界区过宽 -- 中等风险

`partial_update`（handler.rs:93-191）在持有 `GLOBAL_GATEWAY_STORE` 写锁期间调用了多个外部操作：

```
config_store.update_gateway()          // ArcSwap 写 -- 不会 panic
update_gateway_backend_tls_store()     // ArcSwap 写 -- 不会 panic
rebuild_gateway_tls_matcher()          // ArcSwap 写 -- 理论上不会 panic
rebuild_listener_gateway_infos()       // ArcSwap 写 -- 理论上不会 panic
rebuild_all_listener_managers()        // 重建路由管理器
```

虽然这些操作当前不太可能 panic，但临界区越宽，未来引入 panic 的风险越高。如果其中任何一个 panic，`GatewayStore` 就会被毒化。

**对比**：`full_set`（handler.rs:39-63）的做法更好——在写锁块内只做 CRUD 操作，释放锁后再调用 `config_store.full_set()` 等。

**建议**：将 `partial_update` 改为与 `full_set` 相同的模式——先在写锁内完成 store 变更并提取 `list_gateways()` 快照，释放锁后再触发 rebuild。

### 4. 测试代码使用 `unwrap()` -- 可接受

`routes/http/tests.rs` 中 29 处 `store.write().unwrap()` 是测试代码，单线程执行，不会中毒，无需修改。

### 5. 中毒后数据一致性评估

即使 `GatewayStore` 被毒化后通过 `into_inner()` 恢复：

- **最佳情况**：HashMap 在某次 `insert`/`remove` 完成前被中断，但 HashMap 自身仍是有效状态（Rust 的 panic safety 保证 HashMap 在 `insert` panic 后不会出现 dangling entry）
- **实际影响**：某个 Gateway 可能缺失或残留在 store 中，但下一次 `full_set` 会完全重建，自动恢复一致性
- **风险窗口**：两次 `full_set` 之间的 `partial_update` 可能基于不一致数据执行，但路由匹配的实际数据来源是 `ArcSwap` 快照，不直接查 `GatewayStore`

## 最佳处理方案

### 方案对比

| 方案 | 优点 | 缺点 |
|------|------|------|
| **A. 保持现状** (`unwrap_or_else` 恢复) | 零改动，已能正常工作 | 临界区过宽（#2）；语义上承认数据可能不一致 |
| **B. 迁移至 `parking_lot::RwLock`** | 从根本上消除中毒问题；性能略优（无中毒检查开销）；API 更简洁（直接 `.write()` 返回 guard） | 需要改动所有 callsite；与项目其他模块混用两种锁风格 |
| **C. 收窄临界区 + 保持 std 锁** | 减少中毒窗口；`full_set`/`partial_update` 风格统一 | 不消除中毒问题本身 |
| **D. 用 ArcSwap 替代 RwLock** | 与 `GatewayConfigStore`/`ListenerGatewayInfoStore` 风格统一；无锁读取 | `GatewayStore` 写操作需要 clone-modify-swap；语义变化较大 |

### 推荐：B + C 组合

1. **短期 (C)**：收窄 `partial_update` 临界区，使其与 `full_set` 模式一致
2. **中期 (B)**：将 `GLOBAL_GATEWAY_STORE` 迁移至 `parking_lot::RwLock`——项目已依赖 `parking_lot = "0.12"`（见 `Cargo.toml:130`），且 `gateway/cache/lru.rs` 和 `routes/udp/edgion_udp.rs` 已在使用

迁移至 `parking_lot` 的收益：
- `parking_lot::RwLock` 不会中毒——panic 时自动释放锁，guard 析构不标记 poison
- 无需 `unwrap_or_else` 恢复代码，`.write()` 直接返回 `RwLockWriteGuard`
- 微小性能优势（无 poison flag 检查开销）
- 与项目中已有的 `parking_lot` 使用保持一致

**不推荐方案 D**：`GatewayStore` 的写操作（`clear` + 批量 `add_gateway`）如果用 ArcSwap 需要 clone 整个 HashMap 做修改再 swap，开销高于 RwLock，且 `partial_update` 中的多步变更语义更适合 mutable guard。

## 优先级

| 优先级 | 事项 | 工作量 |
|--------|------|--------|
| P2 | 收窄 `partial_update` 临界区（handler.rs:93-191） | 小（约 20 行重构） |
| P3 | 迁移 `GLOBAL_GATEWAY_STORE` 至 `parking_lot::RwLock` | 小（改 1 个 static + 3 个 callsite，删除 `unwrap_or_else`） |
| P4 | 统一项目锁风格（其他模块的 `std::sync::RwLock` → `parking_lot`） | 中（跨模块，可分批） |
| -- | 测试代码 `unwrap()` | 不需改动 |

**总结**：Gateway Runtime 的锁中毒处理在当前实现下是**功能正确**的（写路径有恢复，无 read 路径暴露），主要改进点是 `partial_update` 临界区过宽和统一使用 `parking_lot` 消除中毒问题。
