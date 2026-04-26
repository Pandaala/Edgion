# CPU / Memory 常见误判场景

> 来源：2026-03-30 全项目 CPU/Memory 深度 Review（574 个 Rust 源文件），逐一对照源码核实后的结论。

---

## metrics label `.to_string()` 是 API 强制要求

**误判场景**：批评 `record_backend_request()` / `record_request_duration()` 中每次 `.to_string()` 导致不必要的堆分配。

**实际情况**：`metrics` 0.24 的 label 值类型为 `SharedString = Cow<'static, str>`。动态值（gateway name、namespace、route 等均非 `'static`）**必须** 是 `Cow::Owned(String)`，`.to_string()` 是 API 最小满足形式。`Arc<str>` 和 `&str` 方案均因 lifetime 不满足 `'static` 而不可行。metrics 调用的主要开销来自注册表 hash + atomic，而非字符串分配本身。

**判定**：不是性能问题，无需修复。

---

## `get_raw_any` / `lookup_gateway` 不是 JSON 序列化

**误判场景**：认为 `lookup_gateway()`、`lookup_service_cluster_ip()` 调用 `list_json()` 做全量 JSON 序列化后线性扫描。

**实际情况**：
```rust
// route_utils.rs
pub fn lookup_gateway(namespace: &str, name: &str) -> Option<Arc<Gateway>> {
    let processor = PROCESSOR_REGISTRY.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}
```
`get_raw_any` 走 `KubeRawStore`（DashMap）或 `FsRawStore`（RwLock<HashMap>），均为 **O(1) 查找**。`downcast` 只做 TypeId 比较，无 JSON 序列化，无线性扫描。

**判定**：代码正确高效，不是性能问题。

---

## HTTP/gRPC route 正则匹配：Controller 已校验 + 热路径无 fallback

**历史误判**：`match_header` / `match_query_param` 中曾有 `compiled_regex` 为 `None` 时调用 `Regex::new()` 的回退分支，被审计反复标记为热路径性能问题。

**当时的辩护**：声称"路由创建时已统一预编译，回退分支只在配置 bug 时走，正常运行不触发"。

**辩护的破绽**：Controller 端 `validate_http_route_if_enabled` / `validate_grpc_route_if_enabled` 当时**只校验 `match_type` 枚举值**，没有真正 `Regex::new()` 试编译。非法正则可以畅通进入数据面，`compile_match_regexes` 用 `Regex::new(...).ok()` 静默吞错，最终每次请求落到 fallback 重复编译且重复失败——既隐藏配置错，又付热路径开销。

**当前修复（2026-04-25）**：
1. Controller 侧 `ref_grant/validator.rs::check_regex_pattern` 在 `validate_*_route_if_enabled` 中对所有 `RegularExpression` 字段（HTTPRoute path/header/query, GRPCRoute method.service/method.method/header）`Regex::new()` 试编译，失败 push 到 errors → 路由 `Accepted=False`。
2. Gateway 侧 `match_unit.rs::match_header` / `match_query_param`（HTTP & gRPC）**删除运行时 fallback**，`compiled_regex == None` 直接返回 `Ok(false)` fail-closed。
3. Gateway 侧 `compile_match_regexes` 仍保留 `Regex::new(...).ok()`（防御 controller bug），失败时 `tracing::error!` 大声告警，不静默。

**审查时认定标准**：
- 看到 `match_header` / `match_query_param` 里没有 `Regex::new` 调用 → 已是修复后状态，不是问题。
- 看到 `compile_match_regexes` 里 `Ok/Err` 双臂带 `tracing::error!` → 这是数据面对 controller bug 的最后防线，不是性能问题。
- 看到 controller `check_regex_pattern` 调用 → 说明非法正则在准入期就被拒，Gateway fallback 路径理论不可达。

**判定**：当前实现里 fallback 已彻底移除；任何"建议给 match_header 加运行时 Regex::new"的 finding 都是反向 regression。

---

## Watcher Vec 懒清理足够

**误判场景**：`watchers: RwLock<Vec<WatchClient<T>>>` 在断开连接后不立即缩容，被标记为内存增长。

**实际情况**：每次注册新 watcher 时执行 `retain(|w| !w.sender.is_closed())` 清理已断开的旧 watcher，新旧 watcher 交替时清理及时。Controller 场景 watcher 数量有限（通常 < 100），即使全部积累也不超过 20KB。在 `notify_watchers` 中增加 retain 会导致每次广播都获取 write lock，反而增加热路径锁竞争。

**判定**：懒清理足够，当前设计合理，不是内存问题。

---

## Webhook 先读后截断有 16KB 硬上限保护

**误判场景**：`read_body_limited` 调用 `resp.bytes().await` 先完整读入内存再截断，被描述为"1GB 全量缓冲"。

**实际情况**：
```rust
// 调用处
let max_bytes = runtime.config.max_response_bytes.min(WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES);
pub const WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES: usize = 16384;  // 16KB 硬上限
```
两层 `.min()` 守卫，最坏情况读入并丢弃的额外内存 = 16KB，而非描述中的 1GB。代码注释已承认此限制并说明"sufficient since max_bytes is capped at 16KB"。

**判定**：影响极小（额外 16KB），不需要修复。

---

## OnceLock 全局单例是刻意设计

**误判场景**：`GLOBAL_CONFIG_CLIENT: OnceLock<Arc<ConfigClient>>` 无法释放，被标记为"热重连无法回收旧 ConfigClient"。

**实际情况**：Gateway 进程生命周期内连接的 Controller 端点是固定的，**热切换 Controller 不是当前功能需求**。`OnceLock` 全局单例是刻意的设计决策。若未来需要热重连，需要系统性地重新设计配置同步架构，而不是简单替换为可变状态（会引入大量并发安全问题）。

**判定**：刻意设计，不是内存问题。

---

## Resource reconcile / 配置同步路径的小对象 clone 不是问题

**误判场景**：在 Controller `resource_processor` / Gateway `conf_handler` 等"配置变更触发"的路径上，看到对单个资源、单条 entry、单个 service 的 `.clone()` / `.to_vec()` / 整 HashMap 拷贝，被标为"P3 不必要的 clone / 应改 Arc<T>"。典型样例：

| 位置 | 模式 | 触发频率 |
|------|------|---------|
| `handlers/secret.rs` 同一 secret 给 init_accumulator + secret_store 各 clone 一次 | Secret 资源 reconcile | 每个 Secret 创建/更新 |
| `lb/lb_policy/policy_store.rs::get(service_key) -> Vec<LbPolicy>` | DashMap entry policies `.to_vec()` | HTTPRoute reconcile |

**实际情况**：这些 clone 全部在 reconcile / config sync 路径，调用频率 = K8s 资源变更频率（人/Operator 操作量级，最高也是 reload 时的 burst），**不是每请求触发**。被 clone 的对象通常是单条资源（KB 量级），整路径耗时由 etcd RTT / gRPC 序列化主导，几次 String/HashMap clone 的 alloc + memcpy 在 totals 里测不出来。

要把它们改 Arc 通常需要：
- 改公开类型签名（`WorkItemResult::obj`、`PolicyStore::get` 返回类型等），cascading 到多处 caller / 测试；
- 或加闭包 API（`with_policies(&self, key, F)`）改变上层用法。

冷路径上没有可观测收益，但带来真实的改动面与回归风险。

**判定**：是否在热路径决定一切——
- ✅ 拒绝："P3 reconcile / config sync 路径上的小对象 clone" 类 finding。
- ⚠️ 接受："full_set 整 map 拷贝且每秒同步" 这种放大型场景（已有案例：LinkSys `replace_all` 用 `Arc::clone` 共享给 dispatch task，避免整 HashMap copy；ACME `challenge_store::lookup` 改用 `Bytes` 在 HTTP-01 验证请求每次命中省一次 String alloc + memcpy）。
- 接受口径：finding 必须证明「在请求路径每秒触发」或「单次拷贝 ≥ MB 量级」之一，否则按本条关闭。

---

## 离线 Controller 数据保留是业务需求

**误判场景**：`ControllerRegistry` / `ResourceAggregator` 中离线 Controller 的条目不被定期清理，被标记为"内存泄漏"。

**实际情况**：离线 Controller 的注册信息和资源快照是**需要持久保留的业务数据**——Admin API 需要展示离线状态，运维人员需要查看离线前的资源快照。清理走显式运维路径：`DELETE /api/v1/center/admin/controllers/{id}` 级联删除四个内存结构 + DB 行（见 `skills/09-misc/center-db.md` 的 Admin API 表）。活资源（`stream_tx` / forwarding task / tonic HTTP/2 stream）则在 `mark_offline` 时立即释放，不等 DELETE。

> 历史注意：早期有 `evict_stale(hours)` 方法 + `offline_evict_hours` 配置，但从未被 scheduler 调用，是死代码。已在 2026-04 的 fix #19 中移除；当前语义即"离线数据无 TTL，仅通过 Admin DELETE 清理，但活资源离线瞬间就释放"。

**判定**：非内存泄漏，是业务语义保留；活资源已即时释放，保留的只是静态注册快照。
