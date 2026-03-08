# 路由系统审查

**审查目录**: `src/core/gateway/routes/`
**审查文件**: 51 个 .rs 文件，约 11756 行代码

---

## M-4: HTTP Header/Query 正则匹配每请求编译 Regex [中]

**文件**: `http/match_unit.rs` 第 138-141 行（Header），第 167-170 行（Query）

**问题描述**:

每次请求匹配时，对 `RegularExpression` 类型的 header/query matcher 重新编译 `Regex`。Regex 编译涉及 NFA/DFA 构建的堆分配，在高 QPS 下会产生大量临时堆分配和 CPU 开销。

```rust
// Header 匹配
"RegularExpression" => {
    let re = Regex::new(&header_match.value)  // 每请求编译！
        .map_err(|e| EdError::RouteMatchError(format!("Invalid regex: {}", e)))?;
    Ok(re.is_match(header_value))
}

// Query 匹配
"RegularExpression" => {
    let re = Regex::new(&query_param_match.value)  // 每请求编译！
        .map_err(|e| EdError::RouteMatchError(format!("Invalid regex: {}", e)))?;
    Ok(re.is_match(param_value))
}
```

虽然不是内存泄漏（编译后的 Regex 在作用域结束时释放），但造成：
- 每个候选路由 × 每个正则 matcher → 一次 Regex 编译
- 高 QPS 下大量临时堆分配和 CPU 开销

**对比**: gRPC 代码已有正确的预编译实现：

```rust
// grpc/match_unit.rs:43-70 — 正确做法
let compiled_header_regexes = if let Some(ref headers) = matched.headers {
    headers.iter().map(|header_match| {
        if header_match.match_type.as_deref() == Some("RegularExpression") {
            match Regex::new(&header_match.value) {
                Ok(re) => Some(Arc::new(re)),
                Err(e) => { /* warn */ None }
            }
        } else { None }
    }).collect()
} else { Vec::new() };
```

**建议修复**: 在 `HttpRouteRuleUnit` 或 `MatchInfo` 中增加 `compiled_header_regexes` 和 `compiled_query_param_regexes` 字段，在路由创建时预编译。可直接复用 gRPC 的方案。

---

## L-7: gRPC full_set 双重 Clone [低]

**文件**: `grpc/conf_handler_impl.rs` 第 123-129 行

**问题描述**:

```rust
fn full_set(&self, data: &HashMap<String, GRPCRoute>) {
    let mut parsed_routes = HashMap::new();
    for (key, mut route) in data.clone() {       // Clone #1: 全量克隆
        route.preparse();
        parsed_routes.insert(key, route);
    }
    *self.grpc_routes.lock().unwrap() = parsed_routes.clone();  // Clone #2: 存储时再次克隆
```

导致同一时刻存在 3 份路由数据副本。不是泄漏，但对大量 gRPC 路由会造成瞬时内存峰值。

**建议修复**: 直接使用 `parsed_routes` 而非再次 clone。

---

## 审查通过的子模块

| 子模块 | 审查结论 |
|--------|---------|
| RouteManager 全局单例 | 正确：`full_set` 完整替换 HashMap，`partial_update` 正确删除条目 |
| ArcSwap<DomainRouteRules> 替换 | 正确：旧值通过引用计数自动回收 |
| RadixHostMatchEngine 重建 | 正确：复用未变化域名的 `Arc<RouteRules>` |
| RegexRoutesEngine | 正确：引擎不可变，通过 Arc 管理 |
| HttpRouteRuleUnit 生命周期 | 正确：请求期间 Arc clone 仅增加引用计数 |
| EdgionHttpContext 生命周期 | 正确：per-request 创建和销毁 |
| stage_logs / ctx_map | 无泄漏：with_capacity 初始化，受阶段数/插件数约束 |
| proxy_http 各阶段 | 均审查通过，无累积分配 |
| GrpcMatchEngine | 正确：Header 正则预编译，引擎不可变 |
| partial_update vs full_set | 正确：RCU 模式保证并发安全和旧数据及时回收 |
| TCP/TLS/UDP Route | 与 HTTP/gRPC 模式一致 |
| lb_policy_sync | 正确：`cleanup_lb_policies_for_routes` 清理过期策略 |
