# 内存爆炸问题

> 按严重度排列。每个问题包含文件位置、描述、修复建议。

---

## High

### MEM-01: ForwardAuth 插件无界读取错误响应 Body

**文件**: `src/core/gateway/plugins/http/forward_auth/plugin.rs`
**行号**: 345

**问题**: 当认证服务返回错误响应时，`resp.text().await.unwrap_or_default()` 无任何大小限制地读取完整响应体。恶意或配置错误的认证服务可返回多 GB 响应体，导致 Gateway 进程 OOM。认证服务是外部依赖，不受网关运维直接控制。

**修复建议**: 添加响应体大小限制（如 1 MB），超出则截断或拒绝。使用流式读取替代完整缓冲。

---

### MEM-02: Webhook Resolver 完整缓冲响应体后才截断

**文件**: `src/core/gateway/link_sys/providers/webhook/resolver.rs`
**行号**: 274

**问题**: `read_body_limited` 调用 `resp.bytes().await` 先将完整响应读入内存，然后才截断至 `max_bytes`（16 KB）。Webhook 端点返回 1 GB 响应时，1 GB 会完整加载到内存。代码注释已承认该限制。

**修复建议**: 使用 `response.bytes_stream()` 按 chunk 读取，达到限制后提前终止。

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

## Medium

### MEM-04: ServiceRuntimeState DashMaps 永不缩容

**文件**: `src/core/gateway/lb/runtime_state/mod.rs`
**行号**: 27-29

**问题**: `ewma`、`conn_counts`、`backend_states` DashMap 为每个曾出现的 backend 地址累积条目。`BackendCleaner` 只清理 draining 且连接数为 0 的 backend，不清理已消失 backend 的 EWMA/conn_count 条目。滚动部署频繁创建新 Pod IP 时，stale 条目累积。DashMap shard 不会缩容。

**修复建议**: 添加定期清扫任务，对比当前有效 backend 列表，移除不存在的条目。

---

### MEM-05: SERVICES / RR_CACHE / CH_CACHE 全局 DashMap 无清理

**文件**: `src/core/gateway/lb/runtime_state/mod.rs`
**行号**: 43, 46, 48

**问题**: 已删除 Service 的条目永久留在 `SERVICES` DashMap 中。`RR_CACHE` 和 `CH_CACHE` 仅在 `invalidate_selector_cache()` 被调用时清除。无定期清扫机制。

**修复建议**: 在 ConfHandler 处理 Service 删除事件时同步清理这些全局 map。

---

### MEM-06: Watcher Vec 永不缩容

**文件**: `src/core/controller/conf_sync/cache_server/cache.rs`
**行号**: 247-268

**问题**: `watchers: RwLock<Vec<WatchClient<T>>>` 每次新 Watch 建立时增长。断开的 Watcher 仅在新 Watch 注册时懒清理（retain）。空闲期间无清理。`WatchClient` 含 Arc/RwLock/Sender，非 trivial 大小。频繁 Gateway 重启（滚动更新）时持续增长。

**修复建议**: 添加定期清理任务或在 `notify_watchers()` 中同时做 retain。

---

### MEM-07: Watch 三级 Channel 级联 + 转发 Task

**文件**: `src/core/controller/conf_sync/cache_server/cache.rs`, `grpc_server.rs`

**问题**: 每个 Watch 创建 4 个 mpsc channel（各 capacity=100）和 3 个 forwarding task。
- 18 种资源 × 10 个 Gateway = 720 channel × 4 = 2880 channel
- 每 channel 缓冲 100 条消息，消息含序列化 JSON
- EndpointSlice 等大资源的 channel buffer 可持有大量数据

**修复建议**: 减少 channel 级联层数，考虑直接序列化到 gRPC stream。

---

### MEM-08: Center 无 `evict_stale()` 定期调用

**文件**: `src/core/center/cli/mod.rs`, `src/core/center/fed_sync/registry/mod.rs`, `src/core/center/aggregator/mod.rs`

**问题**: `ControllerRegistry` 和 `ResourceAggregator` 有 `evict_stale(hours)` 方法，但仅在单元测试中调用。启动代码未 spawn 定期 GC 任务。离线 Controller 及其快照数据（含所有 ResourceKey 的 labels/annotations）永久占用内存。

**修复建议**: 在 `EdgionCenterCli::run()` 中添加定期 evict_stale() 调度。

---

### MEM-09: Center pending_commands Map 无 GC

**文件**: `src/core/center/fed_sync/server/mod.rs:50`, `src/core/center/commander/mod.rs:45`

**问题**: `pending_commands: PendingCommandMap` 仅在 `Commander::send_command()` 超时时清理单条。若调用方被取消（HTTP 超时/断连），pending 条目永久泄漏。对比 `pending_lists` 有专门的后台 GC task (scheduler/mod.rs:66-85)。

**修复建议**: 为 `pending_commands` 添加类似的后台 GC task。

---

### MEM-10: List 操作全量快照 Clone + JSON 序列化

**文件**: `src/core/controller/conf_sync/cache_server/store.rs:118-124`
**文件**: `src/core/controller/conf_sync/cache_server/cache.rs:320-324`

**问题**: 每次 `list()` 调用 clone 所有 data HashMap 值到 Vec，再序列化为 JSON String。两份数据同时存在内存中。有 64MB 硬限制保护序列化字符串，但中间 `Vec<T>` clone 无限制。多个 Gateway 并发 list EndpointSlice 时可能消耗数百 MB。

**修复建议**: 添加并发 list 限制或使用流式序列化。

---

## Low

### MEM-11: OIDC Singleflight HashMap 清除风暴

**文件**: `src/core/gateway/plugins/http/openid_connect/openid_impl.rs:~1579-1601`

**问题**: `refresh_singleflight_locks/results` 有 4096 条目上限，达到上限时 `map.clear()` 全部清除。可导致所有活跃 session 同时触发 refresh，形成 thundering herd。

---

### MEM-12: Circuit Breaker 消失 Backend 的 EndpointState 不清理

**文件**: `src/core/gateway/backends/circuit_breaker/store.rs:157-167`

**问题**: 健康的 endpoint 通过 `record_success()` 清理。但被驱逐后永久删除的 Pod 的 `EndpointState` 条目仅在 service config 变更时清理。滚动部署累积 stale 条目。

---

### MEM-13: EventStore data HashMap 不缩容

**文件**: `src/core/controller/conf_sync/cache_server/store.rs:17`

**问题**: 大量资源创建后删除，HashMap 保留已分配的 capacity 不释放。实际影响小，HashMap 分配不会无界增长，只是不回收。

---

### MEM-14: CompressEvent Vec 在 100ms 窗口内无界增长

**文件**: `src/core/gateway/conf_sync/cache_client/cache_data.rs:224-242`

**问题**: `CompressEvent` 的 `events: HashMap<String, Vec<ResourceChange>>` 在 100ms 处理窗口内持续 push。实际只用到最新的变更类型，历史记录不必要。EndpointSlice 突发更新时 Vec 可能临时变大。

---

### MEM-15: OnceLock ConfigClient 无法释放

**文件**: `src/core/gateway/conf_sync/mod.rs:9-19`

**问题**: `GLOBAL_CONFIG_CLIENT` 为 `OnceLock<Arc<ConfigClient>>`，一旦设置永不释放。若将来需要热重连到不同 Controller，旧 ConfigClient（含 18+ ClientCache + 后台 polling task）无法回收。

---

### MEM-16: `edgion_status` / `UpstreamInfo.err` Vec 无上限

**文件**: `src/types/ctx.rs:477,583,612` (edgion_status)
**文件**: `src/types/ctx.rs:328` (err)

**问题**: 每请求 context 的这两个 Vec 无硬上限。被重试次数配置间接限制，但无独立保护。长寿命流式请求中理论上可累积。

**修复建议**: 添加硬上限（如 edgion_status 最多 50 条，err 最多 20 条）。
