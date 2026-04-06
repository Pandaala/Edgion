# 问题标题

**配置同步路径上对 `RwLock` / `Mutex` 加锁结果使用 `.unwrap()` 导致的毒化锁（Poison）恐慌风险**

在多个 `conf_handler` 实现中，对 `std::sync::RwLock::read()` / `write()` 以及 `Mutex::lock()` 的返回值直接调用 `.unwrap()`。当此前持锁线程在临界区内发生 panic 时，锁会被标记为 **poisoned**；此后任何一次 `unwrap()` 都会在获取锁时立刻 panic，导致该 handler 相关的配置更新路径不可用，并可能拖垮整个网关进程。

---

## 严重程度

**中**

- 触发依赖「持锁期间发生 panic」这一相对少见但完全现实的条件（资源解析、新代码缺陷、极端内存压力等）。
- 一旦触发，影响面为**该全局存储 / 缓存对应的整条配置同步链路**，且表现为**持续性失败**（非单次请求重试可恢复），在网关场景下属于不可忽视的可用性风险。
- 与「业务逻辑错误返回 Err」不同，此处为**基础设施级**的硬崩溃路径，运维与监控上更难与业务错误区分。

---

## 受影响文件与具体行号

以下行号以当前仓库快照为准，已与源文件核对。

### 1. `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`

| 行号 | 代码模式 | 说明 |
|------|----------|------|
| 20 | `EDGION_GATEWAY_CONFIG_STORE.read().unwrap()` | 按名查询 `EdgionGatewayConfig` |
| 27 | `EDGION_GATEWAY_CONFIG_STORE.read().unwrap()` | 列出全部配置 |
| 50 | `EDGION_GATEWAY_CONFIG_STORE.write().unwrap()` | `full_set` 全量写缓存 |
| 68 | `EDGION_GATEWAY_CONFIG_STORE.write().unwrap()` | `partial_update` 增量写缓存 |

静态全局：`EDGION_GATEWAY_CONFIG_STORE: LazyLock<Arc<RwLock<Vec<EdgionGatewayConfig>>>>`。

### 2. `src/core/gateway/config/gateway_class/conf_handler_impl.rs`

| 行号 | 代码模式 | 说明 |
|------|----------|------|
| 20 | `GATEWAY_CLASS_STORE.read().unwrap()` | 按名查询 `GatewayClass` |
| 27 | `GATEWAY_CLASS_STORE.read().unwrap()` | 列出全部 `GatewayClass` |
| 50 | `GATEWAY_CLASS_STORE.write().unwrap()` | `full_set` |
| 68 | `GATEWAY_CLASS_STORE.write().unwrap()` | `partial_update` |

静态全局：`GATEWAY_CLASS_STORE: LazyLock<Arc<RwLock<Vec<GatewayClass>>>>`。

### 3. `src/core/gateway/routes/grpc/conf_handler_impl.rs`

| 行号 | 代码模式 | 说明 |
|------|----------|------|
| 62 | `*self.route_units_cache.lock().unwrap() = new_units_cache` | `full_set` 中替换 `route_units_cache` |
| 118 | `let mut units_cache = self.route_units_cache.lock().unwrap()` | `partial_update` 中在锁内更新 units 与 `route_cache` |

`GlobalGrpcRouteManagers` 的 gRPC 路由全量 / 增量同步均依赖该 `Mutex`。

---

## 风险场景分析

### Rust 标准库语义简述

- `std::sync::RwLock` / `Mutex` 在持锁线程 panic 且未正常释放时，会将锁置于 **poisoned** 状态。
- 后续 `read()` / `write()` / `lock()` 返回 `Err(PoisonError<...>)`，对 `Result` 使用 `.unwrap()` 会 **panic**。

### 网关 / 代理场景下的典型触发链

1. **畸形或恶意资源在临界区内触发 panic**  
   例如路由解析、`preparse()`、深层结构假设被违反等。gRPC 路径中 `full_set` 在加 `route_units_cache` 锁**之前**已执行循环内的 `preparse()` 与 `parse_route_to_units`；若未来代码调整顺序或在锁内增加逻辑，毒化风险会直接落在 `Mutex` 上。当前 `partial_update` 已在 `lock()` 之后调用 `parse_route_to_units`，**锁内解析**已存在：若 `parse_route_to_units` 或其依赖 panic，将导致 `route_units_cache` 毒化。

2. **OOM 或分配失败在临界区内表现为 abort/panic（视构建与运行环境）**  
   大规模路由表重建、大量 `clone` / `Arc` 分配时，在持锁段内失败会放大为「锁永久不可用」的系统性问题。

3. **新功能在持锁段内引入 bug**  
   `full_set` / `partial_update` 是高频迭代区域，任何新增的 `unwrap()`、`assert!`、越界或逻辑错误若在锁内触发 panic，后果相同：**单次 bug → 持久性配置管道失效**。

### 影响范围

| 维度 | 说明 |
|------|------|
| **配置同步** | 对应 handler 的 `full_set` / `partial_update` 再次调用时在加锁处 panic，配置无法更新。 |
| **读路径** | `EdgionGatewayConfig` / `GatewayClass` 的查询函数同样 `read().unwrap()`，毒化后读配置也会 panic。 |
| **进程级** | 若配置同步在独立任务中未隔离 panic，或未使用 `catch_unwind`，可能导致工作线程 / 整个进程终止，取决于调用栈与运行时策略。 |
| **级联** | 多个 handler 独立；**单个**毒化锁只影响该资源维度，但网关整体「配置不一致」与「部分路径持续崩溃」仍属严重运维事件。 |

---

## 各修复方案对比

### 方案一：`.expect("...")` 带说明信息

**做法**：将 `.unwrap()` 替换为 `.expect("EDGION_GATEWAY_CONFIG_STORE write lock poisoned: ...")` 等。

| 优点 | 缺点 |
|------|------|
| 改动极小，消息更可读，便于日志定位 | **不消除 panic**，仅改善可观测性 |
| 无新依赖 | 毒化后行为与 `unwrap` 相同：仍崩溃 |

**结论**：可作为「最低限度」的文档化与排障改进，**不能**单独作为高可用网关的终局方案。

### 方案二：匹配 `Err(poisoned)` 并 `into_inner()` 恢复

**做法**：

```rust
let mut store = match EDGION_GATEWAY_CONFIG_STORE.write() {
    Ok(guard) => guard,
    Err(poisoned) => {
        tracing::error!(target: "conf_sync", "EDGION_GATEWAY_CONFIG_STORE poisoned; recovering with into_inner");
        poisoned.into_inner()
    }
};
```

| 优点 | 缺点 |
|------|------|
| **避免在毒化后再次 panic**，配置同步可继续 | 毒化时内部数据可能处于**不一致的中间状态**；`into_inner` 得到的是 panic 前的部分更新状态，需业务上评估是否可接受 |
| 与标准库兼容，不增加依赖 | 应在恢复后考虑 **metrics / 告警**，否则静默吞掉严重事件 |
| 可对读锁 `read()` 同样处理 | gRPC `Mutex` 路径需在锁内更新的语义下，更要明确「恢复后继续」与 `route_cache` 的一致性策略 |

**结论**：在明确「宁愿保留可能不一致的快照、也要保持进程与同步管道存活」的产品假设下，是标准库场景下较务实的工程折中。

### 方案三：`parking_lot::RwLock` / `Mutex`（不毒化）

**做法**：将全局存储或热点锁替换为 `parking_lot` 的实现（其锁在 panic 后**不**采用 std 的 poison 语义，行为与策略需查阅所用版本文档）。

| 优点 | 缺点 |
|------|------|
| Often 更低开销、无 poison 导致的 `Err` 分支 | **语义变更**：团队必须统一理解「panic 后无 poison 标记」与调试含义 |
| 可减少「毒化即崩」这一类失败模式 | 需评估与现有 `Arc<std::sync::RwLock<...>>` 类型边界、FFI、及全项目锁策略的一致性 |
| | 新增或显式依赖 `parking_lot`（若尚未引入）需走依赖与审计流程 |

**结论**：适合作为 **长期架构选项**（尤其高并发配置读路径），但需单独设计文档与评审，不宜仅局部替换而不说明全局锁策略。

### 方案四（补充）：缩小临界区 + 减少锁内 panic 源

**做法**：将解析、校验、构建新快照移到锁外，锁内仅做 `swap` 或短路径赋值。

| 优点 | 缺点 |
|------|------|
| 从源头降低「持锁 panic → 毒化」概率 | 重构成本高于单纯替换 `unwrap` |
| 与方案二 / 三可组合 | gRPC 当前 `partial_update` 结构需仔细设计 `route_cache` 与 `route_units_cache` 的一致性 |

**结论**：与 二 / 三 **组合**最佳，单独作为唯一手段时仍建议保留对 `PoisonError` 的处理或换锁实现。

---

## 推荐方案

1. **短期（必做）**  
   - 在配置同步关键路径上，**停止使用裸 `.unwrap()` 获取 `RwLock` / `Mutex` 守卫**。  
   - 至少采用 **方案二**（`match` + `tracing::error!` + `into_inner()`），并对 `read()` / `write()` / `lock()` **三处文件、所列行号逐一替换**。  
   - 为毒化恢复增加 **metric 或结构化日志字段**（如 `lock_poison_recovered = true`），便于告警。

2. **中期（建议）**  
   - 对 `GlobalGrpcRouteManagers` 的 `partial_update`：**尽量将 `parse_route_to_units` 移出 `Mutex` 临界区**，先在无锁数据结构中建好 `HashMap`，再在极短临界区内 `extend` / 替换，降低毒化概率并与方案二形成纵深防御。

3. **长期（可选）**  
   - 若项目统一引入 `parking_lot` 且接受其语义，可对高频全局配置存储评估 **方案三**；与 **方案四** 结合时，整体健壮性最好。

**不推荐**仅将 `.unwrap()` 改为 `.expect("...")` 作为唯一修复；其仅改善崩溃信息，**不解决配置管道在毒化后持续不可用的问题**。

---

## 参考代码位置（便于跳转）

- `edgion_gateway/conf_handler_impl.rs`：`EDGION_GATEWAY_CONFIG_STORE` 的 `read`/`write` + `unwrap`。  
- `gateway_class/conf_handler_impl.rs`：`GATEWAY_CLASS_STORE` 同上。  
- `routes/grpc/conf_handler_impl.rs`：`impl ConfHandler<GRPCRoute> for GlobalGrpcRouteManagers` 中 `route_units_cache.lock().unwrap()`。

---

## 文档信息

- **审查主题**：RwLock / Mutex 加锁结果 `unwrap` 与毒化锁恐慌风险  
- **严重级别**：中  
- **状态**：待修复（代码审查记录）  
- **关联路径**：配置同步 `ConfHandler`（`full_set` / `partial_update`）及全局查询 API  
