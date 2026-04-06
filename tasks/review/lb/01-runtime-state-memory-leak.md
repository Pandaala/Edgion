# [P1] Runtime State 全局静态变量的内存泄漏风险

## 严重程度定义（本文档约定）

| 级别 | 含义 |
|------|------|
| **P0** | 数据丢失 / 进程崩溃 |
| **P1** | 功能异常 / 内存泄漏（含长期资源无法回收） |
| **P2** | 性能问题（延迟、吞吐、CPU） |
| **P3** | 代码质量、可维护性 |

**本文档归类：P1** — 在动态 K8s 环境下，全局 `DashMap` 与 per-service 内部表可能持续增长且无法与真实后端集合对齐，属于**资源无法回收 + 算法被陈旧数据污染**类问题；一般不直接导致立即崩溃，但可演化为 OOM 或明显错误的负载均衡决策。

---

## 问题描述

负载均衡运行时状态集中在 `runtime_state` 模块中，通过多组**进程级静态** `DashMap` 缓存：

- 每个 Service（`namespace/name`）对应的 `ServiceRuntimeState`（EWMA、连接数、后端生命周期状态）
- `RoundRobinSelector`、`ConsistentHashRing`、Session Persistence 亲和表

上述结构的设计意图是「按 Service / Endpoint 生命周期清理」。实际路径依赖**发现层显式调用** `remove_service` / `remove_backend` / `invalidate_selector_cache`。当事件不完整、仅失效缓存而不收缩 per-backend 表、或后端 IP 高频轮换时，会出现**条目只增不减或长期残留**，表现为内存占用上升与指标/选择逻辑偏离当前集群真相。

---

## 影响范围

| 区域 | 说明 |
|------|------|
| **进程内存** | 所有网关 worker / 单进程内全局表；条目数与「曾出现过的 service_key × SocketAddr」相关 |
| **负载均衡行为** | LeastConn / EWMA 等会读取 `conn_counts`、`ewma`；陈旧条目可能扭曲权重与排序 |
| **会话保持** | `SP_AFFINITY` 有每 service 上限，但与其他表无统一 TTL |
| **控制面 / 数据面一致性** | 依赖 Informer 全量重同步（`full_set`）与增量路径（`partial_update`）均正确调用清理 API |

---

## 代码位置

| 符号 | 文件 |
|------|------|
| `SERVICES`, `RR_CACHE`, `CH_CACHE`, `SP_AFFINITY` | `Edgion/src/core/gateway/lb/runtime_state/mod.rs` |
| `invalidate_selector_cache`, `remove_service`, `remove_backend` | 同上 |
| Endpoint / EndpointSlice 发现层清理 | `.../backends/discovery/endpoint/conf_handler_impl.rs`、`.../endpoint_slice/conf_handler_impl.rs` |
| 仅清理 draining 且 count=0 的后端 | `Edgion/src/core/gateway/lb/leastconn/cleaner.rs`（`BackendCleaner`） |
| Service 资源删除时的 `remove_service` | `.../backends/discovery/services/conf_handler_impl.rs` |

核心静态与缓存 API：

```43:87:Edgion/src/core/gateway/lb/runtime_state/mod.rs
static SERVICES: LazyLock<DashMap<String, Arc<ServiceRuntimeState>>> = LazyLock::new(DashMap::new);

/// Cached `RoundRobinSelector` per service key.
static RR_CACHE: LazyLock<DashMap<String, Arc<RoundRobinSelector>>> = LazyLock::new(DashMap::new);

/// Cached `ConsistentHashRing` per service key.
static CH_CACHE: LazyLock<DashMap<String, Arc<RwLock<ConsistentHashRing>>>> = LazyLock::new(DashMap::new);

/// Session affinity map: token (CRC32 hex) → endpoint SocketAddr, per service key.
/// Built lazily on first session persistence request. Invalidated when backends change.
/// Cap: max 1024 entries per service to prevent memory explosion.
static SP_AFFINITY: LazyLock<DashMap<String, Arc<std::collections::HashMap<String, std::net::SocketAddr>>>> =
    LazyLock::new(DashMap::new);

// ...

pub fn invalidate_selector_cache(service_key: &str) {
    RR_CACHE.remove(service_key);
    CH_CACHE.remove(service_key);
    SP_AFFINITY.remove(service_key);
}
```

---

## 详细分析

### 1. 全局静态 DashMap 累积问题

**机制**：`SERVICES` 在首次访问某 `service_key` 时通过 `get_or_create_service` 创建 `Arc<ServiceRuntimeState>`，且**只有** `remove_service` 会移除整条 service 级记录。

```288:296:Edgion/src/core/gateway/lb/runtime_state/mod.rs
/// Remove the entire service runtime state and selector caches.
pub fn remove_service(service_key: &str) {
    SERVICES.remove(service_key);
    invalidate_selector_cache(service_key);
    tracing::info!(
        service_key = %service_key,
        "Removed service runtime state"
    );
}
```

**风险**：

- K8s 中 Service / Endpoints 频繁增删；若**删除事件未到达**（网络分区、Informer 异常、仅依赖增量而丢 `remove`）、或**增量路径未调用** `remove_service`，则 `SERVICES` 中对应 key 及内部所有 `DashMap` 长期存活。
- 与项目内已有结论一致：`EndpointHandler::partial_update` / `EpSliceHandler::partial_update` 在收到 `remove` 集合时，**未**对移除的 service 执行与 `full_set` 中 `stale_services` 相同的 `remove_service` 清理逻辑；若长期只走增量且某次 `remove` 丢失，可能出现**永久残留**。

**结论**：全局表的生命周期与「发现层是否 100% 调用 `remove_service`」强耦合，缺少兜底清扫时，属于典型 P1 泄漏面。

---

### 2. ServiceRuntimeState 内部 DashMap 累积

**结构**：每个 service 内维护三套以 `SocketAddr` 为键的表：

```24:38:Edgion/src/core/gateway/lb/runtime_state/mod.rs
pub struct ServiceRuntimeState {
    pub ewma: DashMap<SocketAddr, AtomicU64>,
    pub conn_counts: DashMap<SocketAddr, AtomicUsize>,
    pub backend_states: DashMap<SocketAddr, BackendState>,
}
```

**清理路径**：

- `remove_backend` / `remove_backends` 仅在显式调用时删除某一地址上的条目。
- 发现层在 `cleanup_removed_backends` 中：若 `get_count > 0` 则 `mark_backend_draining`，否则 `remove_backend`；`BackendCleaner` 仅对 **draining 且 count==0** 的后端调用 `remove_backend`。

**风险**：

- Pod 重建导致 **新 IP** 不断出现；旧 IP 若从未进入「removed 集合」的 diff（例如 LB 侧与 API 视图短暂不一致），或长期处于 draining 等待连接清零，则 `ewma` / `conn_counts` / `backend_states` 中**旧 SocketAddr 条目**可能长期保留。
- `DashMap` 分片通常不会随删除自动剧烈缩容，表现为常驻堆占用上升。

---

### 3. 缺少 TTL 或主动过期机制

当前无任何「最后访问时间」或「与当前 backend 列表求差后淘汰」的通用机制：

- **无 TTL**：长期运行的网关可能积累大量**已废弃 service** 或**已消失 backend** 的指标，占用内存。
- **算法影响**：EWMA / LeastConn 若仍读到旧地址上的非零计数或非默认 EWMA，可能**错误地参与比较**（取决于上层选择逻辑是否过滤「非当前 backends 列表」）。

因此不仅是「占内存」，还可能归类为**行为偏离**（仍属 P1 范畴）。

---

### 4. `invalidate_selector_cache` 与 `SERVICES` 的不一致性

**行为**：`invalidate_selector_cache` 只移除 `RR_CACHE`、`CH_CACHE`、`SP_AFFINITY`，**不**触及 `SERVICES` 内的 `ewma` / `conn_counts` / `backend_states`。

**与「幽灵后端」的关系**：

- **选择路径**：RR/CH 缓存在失效后会按**当前** `backends` 重建，通常不会把已不在列表中的地址选进新环或新 RR 序列。
- **指标路径**：若选择或健康逻辑仍查询 `get_ewma` / `get_count` / `get_backend_state` 且传入的候选集合与 runtime 表不同步，或存在合并多数据源的代码路径，则**过时的 per-addr 状态**仍可能被读到，形成「列表已无该后端，但状态表仍认为其活跃/有负载」的**语义幽灵**。

**结论**：缓存层与指标层**失效粒度不一致**，依赖调用方在每次后端变更时同时 `invalidate_selector_cache` **且** `remove_backend`/draining 流程完整；任一环节缺失都会放大不一致窗口。

---

### 5. 缓存重建成本（P2 附属）

`get_rr_selector` 与 `get_ch_ring` 使用 `or_insert_with` **惰性构建**：

```64:80:Edgion/src/core/gateway/lb/runtime_state/mod.rs
pub fn get_rr_selector(service_key: &str, backends: &[Backend]) -> Arc<RoundRobinSelector> {
    RR_CACHE
        .entry(service_key.to_string())
        .or_insert_with(|| Arc::new(RoundRobinSelector::build(backends)))
        .value()
        .clone()
}

pub fn get_ch_ring(service_key: &str, backends: &[Backend]) -> Arc<RwLock<ConsistentHashRing>> {
    CH_CACHE
        .entry(service_key.to_string())
        .or_insert_with(|| Arc::new(RwLock::new(ConsistentHashRing::build(backends))))
        .value()
        .clone()
}
```

**风险**（性能项，**P2**）：

- `RoundRobinSelector::build` 若包含排序等 O(n log n) 工作，**首个**命中该 service 的请求可能承担完整构建。
- `ConsistentHashRing::build` 构建 ketama 环，成本更高；在高并发下**多个请求同时** `or_insert_with` 时，可能重复构建（取决于 `DashMap::entry` 语义与实现），造成短暂 CPU 尖峰。

本文档主 severity 仍为 **P1（泄漏/状态陈旧）**；该项作为关联性能风险单独标注。

---

## 复现场景（思路级）

1. **增量 remove 未触发 `remove_service`**  
   仅依赖 Endpoint/EndpointSlice 的 `partial_update`，且某次 `remove` 集合未包含已删除的 service（或 handler 未对 remove 做与 `full_set` 对称的清理）。观察 `runtime_state::all_service_keys()` 仍包含已下线 service。

2. **滚动发布导致 SocketAddr 爆炸**  
   高频 Deployment 滚动，发现层能正确 diff，但大量旧 IP 因 `conn_count > 0` 进入 draining，清理依赖 `BackendCleaner` 周期与连接回收；若连接泄漏或长时间不断开，`remove_backend` 迟迟不执行，`ewma`/`conn_counts` 体积持续上升。

3. **控制面重启 /  watch 断开后的不一致**  
   重连后若某条删除线未重放，全局 `SERVICES` 保留旧 service，内部仍保留历史 backend 指标。

4. **缓存失效后的首包延迟**  
   大批量 `invalidate_selector_cache` 后，多 worker 同时首次访问某热点 service，可观测到 RR/CH 构建带来的延迟尖峰（P2）。

---

## 修改建议

1. **定期扫描（类比 BackendCleaner）**  
   - 周期性遍历 `all_service_keys()`，与**当前发现层权威 backend 集合**（如 RoundRobin store 中的地址集）做差集。  
   - 对「service 已不存在」调用 `remove_service`；对「地址已不在集合中且满足安全条件」调用 `remove_backend` 或进入受控 draining。  
   - 注意与现有 `BackendCleaner` 职责划分，避免重复扫描或锁竞争。

2. **`last_access` + TTL（按 service 或按 backend）**  
   - 在 `ServiceRuntimeState` 上增加 `AtomicU64` 或 `Instant` 序列化的时间戳，在 `get_or_create_service` / 指标更新路径上刷新。  
   - 后台任务淘汰「超过 TTL 未访问且不在当前 backend 列表」的 service 或条目。  
   - TTL 需可配置，并与「长连接服务」场景平衡。

3. **发现变更时统一清理**  
   - 已有 `invalidate_selector_cache`；应扩展为**同一事务性语义**：在 endpoint 变更回调中，对**移除的地址**保证调用 `remove_backend`（或异步队列），对**移除的 service** 在 `partial_update` 的 `remove` 路径与 `full_set` 对齐，**显式** `remove_service` + CB / HC 清理。

4. **`SP_AFFINITY_MAX_PER_SERVICE` 可配置**  
   - 将常量 `1024` 提升为网关配置项，便于在大规模 endpoints 或特殊会话模型下调优。

5. **缓存重建削峰（P2）**  
   - 考虑在后台预热 ring、或对 `get_ch_ring` / `get_rr_selector` 使用「单 flight」构建，避免并发重复 `build`。

---

## 代码示例（示意，非已合并实现）

### 在 partial_update 中补齐与 full_set 对称的 service 移除

```rust
// 伪代码：在 EndpointHandler::partial_update 末尾，对 remove 集合中的 service_key：
for service_key in &remove {
    config_store.set_endpoint_config(service_key, None);
    get_health_check_manager().reconcile_service(service_key);
    runtime_state::remove_service(service_key);
    cb_store.remove_service(service_key);
}
```

需与 `update_data_only` 返回的 `affected_services` 及 health 配置 store 的 key 语义保持一致，避免双删或漏删。

### ServiceRuntimeState 增加最后访问时间（思路）

```rust
pub struct ServiceRuntimeState {
    pub last_access_unix_secs: AtomicU64,
    pub ewma: DashMap<SocketAddr, AtomicU64>,
    pub conn_counts: DashMap<SocketAddr, AtomicUsize>,
    pub backend_states: DashMap<SocketAddr, BackendState>,
}

fn touch_service(svc: &ServiceRuntimeState) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    svc.last_access_unix_secs.store(now, Ordering::Relaxed);
}
```

清扫任务可结合「当前仍存在的 service_key 集合」与 `last_access` 判定是否整表删除。

### 从当前 backends 集合同步清理过时条目（思路）

```rust
pub fn prune_stale_backends(service_key: &str, live: &HashSet<SocketAddr>) {
    let Some(svc) = get_service(service_key) else { return };
    let stale: Vec<SocketAddr> = svc
        .ewma
        .iter()
        .map(|e| e.key().clone())
        .filter(|a| !live.contains(a))
        .collect();
    for a in stale {
        remove_backend(service_key, &a);
    }
}
```

实际生产需与 draining / 连接数约定，避免误删仍有流量的地址。

---

## 小结

| 项 | 结论 |
|----|------|
| **主 severity** | **P1**：内存持续增长 + 指标/状态可能偏离真实后端 |
| **附带** | 缓存惰性重建带来的延迟尖峰为 **P2** |
| **关键点** | 全局表依赖显式清理；`invalidate_selector_cache` 不清理 `SERVICES` 内部 per-addr 表；增量发现路径需与全量路径清理语义对齐 |

建议在实现任何清扫逻辑前，先**统一「权威 backend 集合」来源**（与 discovery store 一致），再在此基础上做 TTL 或差集清理，并补充针对 `partial_update` + 丢事件的集成测试或混沌场景说明。
