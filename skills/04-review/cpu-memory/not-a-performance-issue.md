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

## 编译期预处理的 Regex 回退分支永远不触发

**误判场景**：`match_header` / `match_query_param` 中有 `compiled_regex` 为 `None` 时回退到 `Regex::new()` 的分支。

**实际情况**：`MatchItem` 结构体有 `compiled_header_regexes` / `compiled_query_regexes` 字段，在路由创建时统一预编译。请求匹配时通过索引传入，回退的 `Regex::new()` 分支只在 index 越界（数组长度与 matcher 数量不一致）时触发，属于配置 bug 场景，**正常运行中永远不走**。

**判定**：回退分支是防御性代码，不是热路径，不是性能问题。

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

## 离线 Controller 数据保留是业务需求

**误判场景**：`ControllerRegistry` / `ResourceAggregator` 有 `evict_stale(hours)` 方法但从未被定期调用，被标记为"内存泄漏"。

**实际情况**：离线 Controller 的注册信息和资源快照是**需要持久保留的业务数据**——Admin API 需要展示离线状态，运维人员需要查看离线前的资源快照。`evict_stale()` 适合作为未来手动运维 API 的底层实现，不应自动定时执行（会意外清除有用的历史数据）。

**判定**：非内存泄漏，是业务语义保留，不应自动清理。
