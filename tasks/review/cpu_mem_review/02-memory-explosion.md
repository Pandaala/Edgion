# 内存爆炸问题

> 按严重度排列。每个问题包含文件位置、描述、修复建议。
> **深度复核标注**：每项末尾标注复核结论，含 [确认] / [降级] / [误判] 标记。

---

## High

### MEM-01: ForwardAuth 插件无界读取错误响应 Body

**文件**: `src/core/gateway/plugins/http/forward_auth/plugin.rs`
**行号**: 345

**问题**: 当认证服务返回错误响应时，`resp.text().await.unwrap_or_default()` 无任何大小限制地读取完整响应体。恶意或配置错误的认证服务可返回多 GB 响应体，导致 Gateway 进程 OOM。认证服务是外部依赖，不受网关运维直接控制。

**修复建议**: 添加响应体大小限制（如 1 MB），超出则截断或拒绝。使用流式读取替代完整缓冲。

---

**[深度复核结论] ✅ 确认存在，降级为 Medium（有超时保护）**

**实际代码**：
```rust
// plugin.rs 344-345
// Try to read auth service's response body for error detail
let body = resp.text().await.unwrap_or_default();  // 无 byte 上限
```

**缓解因素**：`timeout_ms`（默认 10 秒）在超时后 `unwrap_or_default()` 返回空字符串，限制了读取时长。但 10 秒内仍可读入数百 MB 数据（100 Mbps 链路），无独立 byte 上限。

**严重度降级**：High → **Medium**（超时提供部分保护，但不充分）

**修复方案（推荐）**：
```rust
// 在 ForwardAuthConfig 增加配置字段
#[serde(default = "default_max_error_body_bytes")]
pub max_error_body_bytes: usize,

fn default_max_error_body_bytes() -> usize { 4096 }  // 默认 4KB

// plugin.rs 替换为流式读取
async fn read_limited_text(resp: reqwest::Response, max_bytes: usize) -> String {
    use futures_util::StreamExt;
    let mut result = Vec::with_capacity(max_bytes.min(4096));
    let mut stream = resp.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        let remaining = max_bytes.saturating_sub(result.len());
        if remaining == 0 { break }
        result.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    String::from_utf8_lossy(&result).into_owned()
}
```

**修复副作用**：超出 4KB 的认证错误响应会被截断，客户端收到不完整 body。可通过 `max_error_body_bytes` 配置调节，或在截断时返回标准错误格式。

---

### ~~MEM-02: Webhook Resolver 完整缓冲响应体后才截断~~

**文件**: `src/core/gateway/link_sys/providers/webhook/resolver.rs`
**行号**: 274

~~**问题**: `read_body_limited` 调用 `resp.bytes().await` 先将完整响应读入内存，然后才截断至 `max_bytes`（16 KB）。Webhook 端点返回 1 GB 响应时，1 GB 会完整加载到内存。~~

---

**[深度复核结论] ❌ 误判，降级为 Low（有硬上限保护），不建议修复**

**实际代码**：
```rust
// resolver.rs 269-285
async fn read_body_limited(resp: reqwest::Response, max_bytes: usize) -> Option<Vec<u8>> {
    // Use bytes() with a size check — reqwest reads the full body
    // For production, we'd use chunk-based reading, but this is sufficient
    // since max_bytes is capped at 16KB.
    match resp.bytes().await {
        Ok(bytes) => {
            if bytes.len() <= max_bytes { Some(bytes.to_vec()) }
            else { Some(bytes[..max_bytes].to_vec()) }
        }
        Err(_) => None,
    }
}
```

```rust
// 调用处（resolver.rs:172）
let max_bytes = runtime.config.max_response_bytes.min(WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES);
pub const WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES: usize = 16384;  // 16KB 硬上限
```

**实际情况**：`max_bytes` 受 `WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES = 16KB` 双重守卫。技术上确实先读全部再截断，但最坏情况下额外内存开销仅为 16KB（读完整体 16KB 后截断），而非 1GB。原描述严重高估风险。代码注释已承认此限制并说明 "this is sufficient since max_bytes is capped at 16KB"。

**结论**：**无需修复**，影响极小。若追求极致可改为流式读取，但性价比极低。

---

### MEM-03: Metrics 标签高基数 + test_data 每请求唯一

**文件**: `src/core/gateway/observe/metrics/registry.rs`
**行号**: 259-284

**问题**: `record_backend_request()` 有 10 个 label 维度（gateway_ns/name, route_ns/name, backend_ns/name, protocol, status, test_key, test_data）。

- **基础问题**: gateway/route/backend/protocol/status 的笛卡尔积创建大量唯一 series，每个 series 永不淘汰
- **严重问题**: 启用 `integration_testing_mode` 时 `test_data` 包含每请求唯一的 JSON（IP、port、hash_key、latency、try_count、error），每请求创建一个新 metric series。遗忘的 `--integration-testing-mode` 标志在生产环境会导致灾难性内存增长

**修复建议**:
- 减少基础 label 维度或合并低基数 label
- `test_data` 不应作为 metric label，改用日志输出
- 添加 series cardinality 上限保护

---

**[深度复核结论] ✅ 确认存在，降级为 Low（仅影响测试模式）**

**实际代码**：
```rust
// pg_logging.rs 148-154（test_data 来源）
let (test_key, test_data) = if crate::core::common::config::is_integration_testing_mode() {
    let key = ctx.gateway_info.metrics_test_key.as_deref().unwrap_or("");
    let data = build_test_data(ctx, error);  // 包含动态 IP/port/hash
    (key, data)
} else {
    ("", String::new())  // ← 生产环境均为空字符串
};
```

**关键发现**：`test_data` 仅在 `is_integration_testing_mode()` 为 true 时填充，**生产环境两个字段均为空字符串 `""`，不会产生高基数**。原描述 "遗忘的标志在生产环境会导致灾难" 有一定道理，但属于运维误操作场景，不是默认风险。

**基础 label 高基数问题**（gateway/route/backend 笛卡尔积）是真实存在的，但这是 metrics 系统的基本权衡，无法通过代码修复。

**修复方案（针对测试模式风险）**：将 `test_data` 从 metric 标签移出，改为日志输出：
```rust
counter!(names::BACKEND_REQUESTS_TOTAL, ..., "test_key" => test_key)
    .increment(1);
// test_data 改为 trace/debug 日志
if !test_data.is_empty() {
    tracing::debug!(test_data = %test_data, "integration test metrics");
}
```

**修复副作用**：测试模式下无法通过 Prometheus 查询 `test_data` 过滤，需从日志关联分析。

---

## Medium

### MEM-04: ServiceRuntimeState DashMaps 永不缩容

**文件**: `src/core/gateway/lb/runtime_state/mod.rs`
**行号**: 27-29

**问题**: `ewma`、`conn_counts`、`backend_states` DashMap 为每个曾出现的 backend 地址累积条目。`BackendCleaner` 只清理 draining 且连接数为 0 的 backend，不清理已消失 backend 的 EWMA/conn_count 条目。滚动部署频繁创建新 Pod IP 时，stale 条目累积。DashMap shard 不会缩容。

**修复建议**: 添加定期清扫任务，对比当前有效 backend 列表，移除不存在的条目。

---

**[深度复核结论] ✅ 部分确认，降级为 Low**

**实际情况**：

清理 API 已完整实现：
```rust
// runtime_state/mod.rs
pub fn remove_backend(service_key: &str, addr: &SocketAddr) {
    if let Some(svc) = get_service(service_key) {
        svc.ewma.remove(addr);
        svc.conn_counts.remove(addr);
        svc.backend_states.remove(addr);
    }
}
```

`BackendCleaner` 在 `cleanup_removed_backends` 中对比新旧 backend 集合，差集即为被移除的 backend，会调用 `remove_backend`。

**真实泄漏点**：仅限 Draining 状态 backend。`mark_backend_draining` 后，若该 backend 连接数自然降为 0 但无 `decrement` 调用路径清理，条目会永久留存。这是小范围的 stale 条目，数量有限（Draining 的 backend 数量 << 总 backend 数量）。

DashMap 容量不缩容是 Rust HashMap 标准语义，不是 bug。

**修复方案（可选）**：在 `decrement` 时检查 Draining 状态并在计数归零时自动清理：
```rust
pub fn decrement(service_key: &str, addr: &SocketAddr) {
    // ...原有逻辑...
    if let Some(svc) = get_service(service_key) {
        if svc.conn_counts.get(addr).map(|c| c.load(Ordering::Relaxed)) == Some(0) {
            if get_backend_state(service_key, addr) == BackendState::Draining {
                remove_backend(service_key, addr);
            }
        }
    }
}
```

**修复副作用**：`decrement` 热路径增加一次状态检查，影响极小。

---

### MEM-05: SERVICES / RR_CACHE / CH_CACHE 全局 DashMap 无清理

**文件**: `src/core/gateway/lb/runtime_state/mod.rs`
**行号**: 43, 46, 48

**问题**: 已删除 Service 的条目永久留在 `SERVICES` DashMap 中。`RR_CACHE` 和 `CH_CACHE` 仅在 `invalidate_selector_cache()` 被调用时清除。无定期清扫机制。

**修复建议**: 在 ConfHandler 处理 Service 删除事件时同步清理这些全局 map。

---

**[深度复核结论] ✅ 确认存在，评级 Medium**

**实际情况**：

`full_set` 路径中有正确的 stale service 清理（`remove_service`），`invalidate_selector_cache()` 在所有 backend 变更（add/remove/update）时均被调用，覆盖正确。

**真实问题路径**：`partial_update` 的 `remove` 集合处理缺少 `remove_service` 调用。Service 被删除时：
- `full_set` 下次执行时会清理（可能延迟较长）
- `partial_update` 路径中 `remove` 集合只处理了 backend 端的 `update_affected_lbs`，**没有显式调用 `remove_service`**

导致：已删除 Service 在 `SERVICES`、`RR_CACHE`、`CH_CACHE`、`SP_AFFINITY` 中的条目永远残留（直到下次 full_set）。

**修复方案**：在 EndpointHandler 的 `partial_update` 中处理 remove 集合时同步清理：
```rust
fn partial_update(&self, ..., remove: HashSet<String>) {
    // ...现有逻辑...
    // 新增：清理被删除的 service
    for service_key in &remove {
        runtime_state::remove_service(service_key);
    }
}
```

**修复副作用**：无负面影响，`remove_service` 是幂等操作。

---

### MEM-06: Watcher Vec 永不缩容

**文件**: `src/core/controller/conf_sync/cache_server/cache.rs`
**行号**: 247-268

**问题**: `watchers: RwLock<Vec<WatchClient<T>>>` 每次新 Watch 建立时增长。断开的 Watcher 仅在新 Watch 注册时懒清理（retain）。空闲期间无清理。`WatchClient` 含 Arc/RwLock/Sender，非 trivial 大小。频繁 Gateway 重启（滚动更新）时持续增长。

**修复建议**: 添加定期清理任务或在 `notify_watchers()` 中同时做 retain。

---

**[深度复核结论] ❌ 基本无问题，不建议修复**

**实际代码**：
```rust
// cache.rs 249-267（注册新 watcher 时的懒清理）
let mut watchers = self.watchers.write().unwrap();
let len_before = watchers.len();
watchers.retain(|w| !w.sender.is_closed());
let len_after = watchers.len();
```

**实际情况**：
1. 每次注册新 watcher 时都会执行 `retain` 清理已断开的旧 watcher，新旧 watcher 交替时清理及时
2. Controller 场景 watcher 数量有限（通常 < 100），每个 `WatchClient` 约 100-200 bytes，即使全部积累也不到 20KB
3. Vec 容量不收缩是 Rust 标准行为，不是 bug；实际 `len` 在 retain 后减少，内存占用与实际 watcher 数量匹配

**结论**：懒清理足够，当前设计合理。若在 `notify_watchers` 中增加 retain 会导致每次广播都获取 write lock，反而增加热路径锁竞争。**无需修复**。

---

### MEM-07: Watch 三级 Channel 级联 + 转发 Task

**文件**: `src/core/controller/conf_sync/cache_server/cache.rs`, `grpc_server.rs`

**问题**: 每个 Watch 创建 4 个 mpsc channel（各 capacity=100）和 3 个 forwarding task。
- 18 种资源 × 10 个 Gateway = 720 channel × 4 = 2880 channel
- 每 channel 缓冲 100 条消息，消息含序列化 JSON
- EndpointSlice 等大资源的 channel buffer 可持有大量数据

**修复建议**: 减少 channel 级联层数，考虑直接序列化到 gRPC stream。

---

**[深度复核结论] ✅ 部分确认，降级为 Low（内存被高估）**

**实际情况**：

每个 watcher 创建 2 个 channel（typed + simple），而非 4 个。Tokio bounded channel **不预分配 slot**，空 channel 约 ~1KB 开销。实际内存占用远低于描述中的数字：
- 10 个 Gateway × 18 种资源 × 2 channel = 360 个 channel
- 空 channel 占用：360KB（远非数百 MB）

**真正的风险**：若 gateway 消费慢而控制器生产快（如批量同步期间），channel 积压 100 条消息时，大资源（EndpointSlice）每条消息的 JSON 可达数十 KB，100 条 × 360 channel = 数百 MB。但这是暂时的积压峰值，不是稳态内存泄漏。

**结论**：正常稳态下内存影响可忽略。仅在同步风暴场景下有峰值压力，可通过减少 channel capacity（如 10→50）或流控来缓解。**Low 优先级，Optional**。

---

### MEM-08: Center 无 `evict_stale()` 定期调用

**文件**: `src/core/center/cli/mod.rs`, `src/core/center/fed_sync/registry/mod.rs`, `src/core/center/aggregator/mod.rs`

**问题**: `ControllerRegistry` 和 `ResourceAggregator` 有 `evict_stale(hours)` 方法，但仅在单元测试中调用。启动代码未 spawn 定期 GC 任务。离线 Controller 及其快照数据（含所有 ResourceKey 的 labels/annotations）永久占用内存。

**修复建议**: 在 `EdgionCenterCli::run()` 中添加定期 evict_stale() 调度。

---

**[深度复核结论] ✅ 确认存在，评级 Medium，建议修复**

**代码确认**：`evict_stale()` 实现完整存在，但整个启动代码（`cli/mod.rs`）中无任何定期调用，仅单元测试中使用。

**修复方案（简单，一次性改动）**：

```rust
// cli/mod.rs 的 run() 中增加
let evict_registry = registry.clone();
let evict_aggregator = aggregator.clone();
let evict_hours = config.sync.offline_evict_hours;  // 新增配置，默认 24
let _evict_handle = tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    interval.tick().await;  // skip first tick
    loop {
        interval.tick().await;
        evict_registry.evict_stale(evict_hours);
        evict_aggregator.evict_stale(evict_hours);
        tracing::info!(component = "center", "Completed stale controller eviction");
    }
});
```

同时在 `CenterSyncConfig` 中增加：
```rust
#[serde(default = "default_offline_evict_hours")]
pub offline_evict_hours: u64,
fn default_offline_evict_hours() -> u64 { 24 }
```

**修复副作用**：
- eviction task 极轻量（每小时一次 HashMap retain），零性能影响
- 新增配置字段需要文档更新
- 长期离线但未正式删除的 Controller 数据会在 24 小时后被清除（符合预期行为）

---

### MEM-09: Center pending_commands Map 无 GC

**文件**: `src/core/center/fed_sync/server/mod.rs:50`, `src/core/center/commander/mod.rs:45`

**问题**: `pending_commands: PendingCommandMap` 仅在 `Commander::send_command()` 超时时清理单条。若调用方被取消（HTTP 超时/断连），pending 条目永久泄漏。对比 `pending_lists` 有专门的后台 GC task (scheduler/mod.rs:66-85)。

**修复建议**: 为 `pending_commands` 添加类似的后台 GC task。

---

**[深度复核结论] ✅ 确认存在（channel-dropped 路径），降级为 Low，可一行修复**

**实际代码**：
```rust
// commander/mod.rs
tokio::time::timeout(self.timeout, rx)
    .await
    .map_err(|_| {
        self.pending.lock().unwrap().remove(&request_id);  // 超时时清理 ✓
        anyhow!("Command timed out after {}s", self.timeout.as_secs())
    })?
    .map_err(|_| anyhow!("Command response channel dropped"))  // ← 这里缺少 remove！
```

**泄漏路径**：当 `oneshot::Sender` 被 drop（控制器连接中断）但 timeout 未到时，`rx.await` 立刻返回 `Err(RecvError)`，走最后的 `map_err` 路径，但该路径**没有 `remove(&request_id)`**。

由于 Sender 已 drop，该条目既不会被 response 路径清理（`server/mod.rs:204`），也不会被 timeout 路径清理（timeout 已不会触发），永久泄漏。

**最小修复（一行改动）**：
```rust
.map_err(|_| {
    // oneshot sender dropped (controller stream closed); clean up pending entry
    self.pending.lock().unwrap().remove(&request_id);
    anyhow!("Command response channel dropped")
})
```

**修复副作用**：无，额外的 `remove` 是幂等操作。

---

### MEM-10: List 操作全量快照 Clone + JSON 序列化

**文件**: `src/core/controller/conf_sync/cache_server/store.rs:118-124`
**文件**: `src/core/controller/conf_sync/cache_server/cache.rs:320-324`

**问题**: 每次 `list()` 调用 clone 所有 data HashMap 值到 Vec，再序列化为 JSON String。两份数据同时存在内存中。有 64MB 硬限制保护序列化字符串，但中间 `Vec<T>` clone 无限制。多个 Gateway 并发 list EndpointSlice 时可能消耗数百 MB。

**修复建议**: 添加并发 list 限制或使用流式序列化。

---

**[深度复核结论] ✅ 确认存在，64MB 保护不存在，评级 Medium**

**实际代码**：
```rust
// store.rs 118-124
pub fn snapshot_owned(&self) -> (Vec<T>, u64) where T: Clone {
    let data = self.data.values().cloned().collect();  // 全量 clone，无大小限制
    (data, self.sync_version)
}
```

**关键发现**：代码中**没有找到 64MB 大小限制保护**。原描述中提到的保护不存在（可能是对另一处限制的误引用）。

**实际风险**：
- K8s 资源（Route 约 10KB，EndpointSlice 可达 500KB）
- N 个并发 list × 资源总大小 = 峰值内存使用
- relist 风暴（controller 重启时所有 gateway 同时发起）可导致数百 MB 峰值

**修复方案**：
1. 添加并发 list 限制（Semaphore）：
```rust
static LIST_SEMAPHORE: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(4));  // 最多 4 并发

pub async fn list_owned_guarded(&self) -> ListData<T> {
    let _permit = LIST_SEMAPHORE.acquire().await.unwrap();
    // ...原有逻辑
}
```

2. 或改为流式序列化（持有读锁期间直接序列化，避免 Vec clone）：
```rust
pub fn list_json(&self) -> Result<(String, u64), String> {
    let store = self.store.read().unwrap();
    let json = serde_json::to_string(&store.data.values().collect::<Vec<_>>())
        .map_err(|e| e.to_string())?;
    Ok((json, store.sync_version))
}
```

**修复副作用**：
- Semaphore 方案：list 请求需排队，对 relist 风暴有明显效果，但正常 list 增加轻微延迟
- 流式序列化：持有读锁时间略长（序列化期间），但避免了 Vec clone，整体性能更好

---

## Low

### MEM-11: OIDC Singleflight HashMap 清除风暴

**文件**: `src/core/gateway/plugins/http/openid_connect/openid_impl.rs:~1579-1601`

**问题**: `refresh_singleflight_locks/results` 有 4096 条目上限，达到上限时 `map.clear()` 全部清除。可导致所有活跃 session 同时触发 refresh，形成 thundering herd。

---

**[深度复核结论] ✅ 确认存在，升级为 High（语义错误 Bug）**

**实际代码**：
```rust
// openid_impl.rs 1577-1605
pub(super) async fn get_or_create_refresh_lock(&self, lock_key: &str) -> Arc<Mutex<()>> {
    let mut locks = self.refresh_singleflight_locks.lock().await;
    if locks.len() > 4096 {
        locks.clear();  // ← 丢弃所有正在飞行中的锁！
    }
    locks.entry(lock_key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub(super) async fn put_refresh_result(&self, lock_key: &str, payload: OidcSessionCookie) {
    let mut results = self.refresh_singleflight_results.lock().await;
    if results.len() > 4096 {
        results.clear();  // ← 正在等待结果的请求失效！
    }
    results.insert(...);
}
```

**严重性**：
1. `locks.clear()` 后：
   - 持有旧锁对象的 goroutine 仍在等待
   - 新请求创建新锁，与旧锁不是同一对象，互斥语义被破坏
   - 多个并发 refresh 请求不再互斥，同时触发 token refresh
2. `results.clear()` 后：
   - 等待 `try_get_recent_refresh_result` 的所有请求重新发起 refresh
3. 对 OIDC Provider 造成突发请求（thundering herd），可能触发 rate limit

**修复方案（推荐）**：

对 `locks` 使用弱引用检测无等待者的锁：
```rust
if locks.len() > 4096 {
    // 只清理没有其他持有者的锁（Map 之外无人持有）
    locks.retain(|_, v| Arc::strong_count(v) > 1);
    // 如果还是 > 4096，说明有大量活跃 session，可以记录告警
}
```

对 `results` 使用基于时间的清理：
```rust
if results.len() > 4096 {
    let threshold = Instant::now() - Duration::from_secs(10);
    results.retain(|_, v| v.created_at > threshold);
}
```

（需要为 result 类型增加 `created_at: Instant` 字段）

**修复副作用**：
- `retain` 是 O(n)，但比 `clear` + thundering herd 代价小得多
- `Arc::strong_count` 不是原子快照，存在 TOCTOU，但足够用于近似判断
- 如果真有超过 4096 个不同用户同时刷新 token，需要 OIDC Provider 端的速率保护

---

### MEM-12: Circuit Breaker 消失 Backend 的 EndpointState 不清理

**文件**: `src/core/gateway/backends/circuit_breaker/store.rs:157-167`

**问题**: 健康的 endpoint 通过 `record_success()` 清理。但被驱逐后永久删除的 Pod 的 `EndpointState` 条目仅在 service config 变更时清理。滚动部署累积 stale 条目。

---

**[深度复核结论] ✅ 部分确认，降级为 Low**

**实际代码**：

正常清理路径完整：
```rust
// 成功时立即 remove（注释明确说明是为了防止内存增长）
pub fn record_success(&self, service_key: &str, backend_hash: u64) {
    svc.endpoints.remove(&backend_hash);  // ← 正确
}

// backend 从 K8s 删除时
pub fn remove_backend(&self, service_key: &str, addr: ...) {
    svc.endpoints.remove(&hash);  // ← 正确
}
```

**真实泄漏点（小）**：在 `is_ejected` 超时恢复时只清零字段，不 remove 条目：
```rust
// 超时恢复后只清零，不 remove
ep_mut.ejected_at = None;
ep_mut.consecutive_failures = 0;
// 此后若该 backend 再无请求（被 LB 排除），条目永久留存（但字段已清零）
```

此场景需要精确的时序：backend 被 eject → eject 超时 → `is_ejected` 清零状态 → backend 从未再收到请求 → 条目残留。数量非常有限。

**修复方案（Optional，一行改动）**：
```rust
// is_ejected 超时恢复时直接 remove
drop(ep);
svc.endpoints.remove(&backend_hash);  // 直接删除而非清零
return false;
```

**修复副作用**：下次该 backend 失败时重新创建条目（`or_default()`），开销极小。

---

### MEM-13: EventStore data HashMap 不缩容

**文件**: `src/core/controller/conf_sync/cache_server/store.rs:17`

**问题**: 大量资源创建后删除，HashMap 保留已分配的 capacity 不释放。实际影响小，HashMap 分配不会无界增长，只是不回收。

---

**[深度复核结论] ✅ 确认存在，评级 Low，不建议修复**

Rust HashMap 不缩容是标准行为。已删除条目的 capacity 留存直到整个 HashMap 被重置。在资源频繁创建/删除场景（如大量临时 Pod）下，capacity 会积累。但容量上限是历史最大 key 数量 × 负载因子，不会无界增长。调用 `shrink_to_fit()` 有摊销成本，且可能造成频繁再分配。**无需修复**。

---

### MEM-14: CompressEvent Vec 在 100ms 窗口内无界增长

**文件**: `src/core/gateway/conf_sync/cache_client/cache_data.rs:224-242`

**问题**: `CompressEvent` 的 `events: HashMap<String, Vec<ResourceChange>>` 在 100ms 处理窗口内持续 push。实际只用到最新的变更类型，历史记录不必要。EndpointSlice 突发更新时 Vec 可能临时变大。

---

**[深度复核结论] ✅ 确认存在，评级 Low，可选优化**

100ms 的事件压缩窗口内，同一 key 的多个事件会被累积到 Vec。处理时只取最后一个类型（压缩的目的）。

**修复方案（简单）**：只保留每个 key 的最新事件，用 `HashMap<String, ResourceChange>` 替代 `HashMap<String, Vec<ResourceChange>>`：
```rust
// 只保留最新的变更类型
events.insert(key, change);  // 替代 events.entry(key).or_default().push(change)
```

但需确认是否所有历史记录都无用（例如先 Delete 后 Add，应保留为 Add，逻辑需要仔细验证）。**Optional**。

---

### MEM-15: OnceLock ConfigClient 无法释放

**文件**: `src/core/gateway/conf_sync/mod.rs:9-19`

**问题**: `GLOBAL_CONFIG_CLIENT` 为 `OnceLock<Arc<ConfigClient>>`，一旦设置永不释放。若将来需要热重连到不同 Controller，旧 ConfigClient（含 18+ ClientCache + 后台 polling task）无法回收。

---

**[深度复核结论] ❌ 设计合理，不建议修复**

`OnceLock` 全局单例是**刻意的设计决策**。Gateway 进程生命周期内连接的 Controller 端点是固定的，热切换到不同 Controller 不是当前功能需求。如果未来需要热重连，需要系统性地重新设计配置同步架构，而不是简单地将 `OnceLock` 替换为可变状态（会引入大量并发安全问题）。**无需修复**。

---

### MEM-16: `edgion_status` / `UpstreamInfo.err` Vec 无上限

**文件**: `src/types/ctx.rs:477,583,612` (edgion_status)
**文件**: `src/types/ctx.rs:328` (err)

**问题**: 每请求 context 的这两个 Vec 无硬上限。被重试次数配置间接限制，但无独立保护。长寿命流式请求中理论上可累积。

**修复建议**: 添加硬上限（如 edgion_status 最多 50 条，err 最多 20 条）。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

两个 Vec 被重试次数配置间接限制（正常请求不超过 10 次重试），边界明确。长寿命流式请求中理论上的累积风险极小。

**修复方案（可选，防御性）**：
```rust
pub fn push_edgion_status(&mut self, status: EdgionStatus) {
    if self.edgion_status.len() < 50 {  // 硬上限
        self.edgion_status.push(status);
    }
}

pub fn push_err(err: UpstreamError) {
    if self.err.len() < 20 {
        self.err.push(err);
    }
}
```

**修复副作用**：超出上限的状态/错误信息会被丢弃，日志可能不完整。建议在丢弃时记录 warn 日志。
