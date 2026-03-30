# CPU / Memory / Infinite Loop 深度 Review 总结

> 审查日期: 2026-03-30
> 审查范围: 全项目 574 个 Rust 源文件 (Controller / Gateway / gRPC Sync / Common / Plugins / LinkSys / Center)

## 统计总览

| 分类 | Critical | High | Medium | Low | 合计 |
|------|----------|------|--------|-----|------|
| CPU 热点 | 1 | 4 | 13 | 7 | 25 |
| 内存爆炸 | 0 | 3 | 7 | 6 | 16 |
| 死循环 | 0 | 0 | 0 | 0 | 0 |
| **合计** | **1** | **7** | **20** | **13** | **41** |

## 结论

- **未发现死循环风险**: TriggerChain 有环检测 (max_depth=20, max_cycles=5)，所有 loop 都有明确退出条件，DSL VM 有 step/call/loop budget。
- **CPU 热点集中在两类模式**: (1) 请求级 Regex 编译 (Critical/High); (2) JSON 序列化/反序列化做缓存查询 (High)。
- **内存爆炸集中在三类模式**: (1) 无界 HTTP Body 读取; (2) Metrics 标签爆炸; (3) DashMap/HashMap 无缩容/无清理。

## 文件索引

| 文件 | 内容 |
|------|------|
| [01-cpu-hotspots.md](01-cpu-hotspots.md) | CPU 热点问题 (25 项) |
| [02-memory-explosion.md](02-memory-explosion.md) | 内存爆炸问题 (16 项) |
| [03-no-infinite-loops.md](03-no-infinite-loops.md) | 死循环审查结论 (无问题) |

## Top 10 优先修复项

| # | 严重度 | 分类 | 位置 | 问题 |
|---|--------|------|------|------|
| 1 | Critical | CPU | `plugins/runtime/conditions/evaluator.rs` | 条件求值器回退路径每请求编译 Regex |
| 2 | High | CPU | `plugins/http/ctx_set/plugin.rs:214` | ctx_set 插件 `apply_transform` 每请求编译 Regex |
| 3 | High | CPU | `plugins/http/dsl/lang/builtins.rs:384,404` | DSL 内置函数 `regex_find`/`regex_replace` 每次调用编译 Regex |
| 4 | High | CPU | `controller/conf_mgr/.../route_utils.rs:421-430` | `lookup_gateway()` 将全部 Gateway JSON 序列化→反序列化→线性扫描 |
| 5 | High | Memory | `plugins/http/forward_auth/plugin.rs:345` | ForwardAuth 无界读取错误响应 Body |
| 6 | High | Memory | `link_sys/providers/webhook/resolver.rs:274` | Webhook resolver 完整缓冲响应体后才截断 |
| 7 | High | Memory | `gateway/observe/metrics/registry.rs:259-284` | Metrics 标签高基数 (test_data 更是每请求唯一) |
| 8 | Medium | CPU | `common/matcher/host_match/radix_match/` | hostname 双重反转，每次都做 Vec+String 分配 |
| 9 | Medium | CPU | `gateway/routes/http/proxy_http/pg_request_filter.rs` | 全局插件每请求 `format!()` + DashMap lookup |
| 10 | Medium | Memory | `center/` | Center 无 `evict_stale()` 定期调用，离线 Controller 数据永不回收 |
