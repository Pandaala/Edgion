# CPU / Memory / Infinite Loop 深度 Review 总结

> 审查日期: 2026-03-30
> 深度复核日期: 2026-03-30（逐一对照源码重新核实）
> 审查范围: 全项目 574 个 Rust 源文件 (Controller / Gateway / gRPC Sync / Common / Plugins / LinkSys / Center)

## 统计总览（深度复核后）

| 分类 | Critical | High | Medium | Low | 虚报/无需修复 | 合计 |
|------|----------|------|--------|-----|-------------|------|
| CPU 热点 | 1 | 3→**1** | 13→**10** | 7→**6** | **5** | 25 |
| 内存爆炸 | 0 | 3→**1** | 7→**5** | 6→**5** | **4** | 16 |
| 死循环 | 0 | 0 | 0 | 0 | — | 0 |
| **合计** | **1** | **2** | **15** | **11** | **9** | **41** |

> **虚报/无需修复**：CPU-04、CPU-05（原 High，代码正确高效）；CPU-13（原 Medium，预编译机制完善）；MEM-02（原 High，有 16KB 硬上限保护）；MEM-15（OnceLock 是刻意设计）等 9 项经查实为误判或优先级明显偏高。

## 结论（更新）

- **未发现死循环风险**：TriggerChain 有环检测 (max_depth=20, max_cycles=5)，所有 loop 都有明确退出条件，DSL VM 有 step/call/loop budget。
- **CPU-01 是唯一 Critical Bug**：`compiled_regex` 字段被 `#[serde(skip)]` 标记，整个生产代码路径从未调用 `.compile()`，所有含 regex 条件的插件**每请求重新编译正则**，必须立即修复。
- **CPU-04 / CPU-05 原分析错误**：`get_raw_any` 实际走 DashMap O(1) 查找 + Arc downcast（仅 TypeId 比较），没有 JSON 序列化，没有线性扫描，代码正确高效，**无需修复**。
- **MEM-11 是高优先级内存 Bug**：OIDC singleflight 使用 `clear()` 全量清空，破坏互斥语义，可引发对 OIDC Provider 的 thundering herd，需要修复。
- **内存爆炸集中在两类模式**：(1) 无界读取（MEM-01）；(2) GC 任务缺失（MEM-05 partial_update 未清理、MEM-08 evict_stale 从未调用）。

## 文件索引

| 文件 | 内容 |
|------|------|
| [01-cpu-hotspots.md](01-cpu-hotspots.md) | CPU 热点问题 (25 项，含深度复核结论) |
| [02-memory-explosion.md](02-memory-explosion.md) | 内存爆炸问题 (16 项，含深度复核结论) |
| [03-no-infinite-loops.md](03-no-infinite-loops.md) | 死循环审查结论 (无问题) |

## 深度复核后修复优先级

### 必须修复（Critical/High + 代码确认）

| # | 严重度 | 分类 | 位置 | 问题 | 复核结论 |
|---|--------|------|------|------|---------|
| 1 | **Critical** | CPU | `plugins/runtime/conditions/evaluator.rs` | `#[serde(skip)]` 导致 regex 从不预编译，每请求 `Regex::new()` | **代码已确认，生产路径从未调用 `.compile()`** |
| 2 | **High** | Memory | `plugins/http/openid_connect/openid_impl.rs` | `locks.clear()` 破坏 singleflight 语义，引发 thundering herd | **代码已确认，`clear()` 丢弃正在飞行中的锁** |

### 强烈建议修复（High/Medium + 代码确认）

| # | 严重度 | 分类 | 位置 | 问题 | 复核结论 |
|---|--------|------|------|------|---------|
| 3 | **High** | CPU | `plugins/http/ctx_set/plugin.rs:214` | transform.replace 路径每请求编译 Regex（extract 路径已预编译） | **代码已确认，transform regex 未缓存** |
| 4 | **Medium** | CPU | `gateway/observe/metrics/registry.rs` | 每请求 10-18 次 `.to_string()` 堆分配 | **代码已确认，真正热路径** |
| 5 | **Medium** | CPU | `gateway/routes/http/proxy_http/pg_request_filter.rs` | 全局插件每请求 `format!()` + DashMap lookup | **代码已确认** |
| 6 | **Medium** | Memory | `plugins/http/forward_auth/plugin.rs:345` | ForwardAuth 无界读取错误响应 Body（降级，有 timeout 保护） | **代码已确认，无 byte 上限** |
| 7 | **Medium** | Memory | `lb/runtime_state/mod.rs` | partial_update remove 路径未调用 `remove_service`，Service 删除后 SERVICES 等 DashMap 条目永久残留 | **代码已确认，真实泄漏路径** |
| 8 | **Medium** | Memory | `controller/conf_sync/cache_server/store.rs` | List 全量 Clone 无并发/大小限制（原描述的 64MB 保护不存在） | **代码已确认** |
| 9 | **Medium** | Memory | `center/cli/mod.rs` | Center `evict_stale()` 实现存在但从未被调用 | **代码已确认，启动代码无调用** |

### 建议修复（Medium/Low，代码确认）

| # | 严重度 | 分类 | 位置 | 问题 |
|---|--------|------|------|------|
| 10 | Medium | CPU | `plugins/http/dsl/lang/builtins.rs` | DSL regex 每次调用编译（需 LRU 缓存）|
| 11 | Medium | CPU | `gateway/observe/access_log/entry.rs` | 每请求 JSON 序列化 + 不必要的 `.clone()` |
| 12 | Medium | Memory | `center/fed_sync/server/mod.rs` | pending_commands channel-dropped 路径缺少 remove（一行修复）|
| 13 | Low | CPU | `gateway/conf_sync/cache_client/cache_data.rs` | 100ms 轮询循环可改为 Notify 唤醒 |
| 14 | Low | CPU | `types/resource/kind.rs` | `from_content()` 每次调用编译 Regex，可用 `LazyLock` |
| 15 | Low | CPU | `common/matcher/host_match/radix_match/` | hostname 双重反转，低成本修复 |
| 16 | Low | Memory | `gateway/backends/circuit_breaker/store.rs` | 超时恢复后 backend 条目不清理（optional）|

### 原分析有误，无需修复

| # | 原严重度 | 分类 | 问题 | 实际情况 |
|---|---------|------|------|---------|
| CPU-04 | High | CPU | `lookup_gateway()` 全量 JSON 序列化→扫描 | **代码正确**：`get_raw_any` 是 DashMap O(1) 查找 + Arc downcast |
| CPU-05 | High | CPU | `lookup_service_cluster_ip()` 同样模式 | **代码正确**：同上 |
| CPU-13 | Medium | CPU | `match_header/match_query_param` 回退 Regex | **预编译机制完善**：正常路径不走回退分支 |
| MEM-02 | High | Memory | Webhook 响应先读全部再截断 | **影响极小**：已有 16KB 硬上限，最坏情况额外 16KB |
| MEM-06 | Medium | Memory | Watcher Vec 永不缩容 | **懒清理足够**：注册新 watcher 时 retain，watcher 数量有限 |
| MEM-15 | Low | Memory | OnceLock ConfigClient 无法释放 | **刻意设计**：全局单例，热重连非当前功能需求 |
