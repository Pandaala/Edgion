# Hostname 重构 — 完整代码 Review 文档

> 基于 `git diff` 的所有改动梳理。涉及 17 个源码文件（含 2 个新增）+ 2 个测试基础设施文件。
> 集成测试结果：**59/59 全部通过 (100%)**

---

## 改动总览

| # | Task | 涉及文件 | 改动量 | 说明 |
|---|------|---------|--------|------|
| T1 | 类型层：新增 `resolved_hostnames` | `http_route.rs`, `grpc_route.rs` | +10 | Route 资源新增控制面预计算字段 |
| T2 | 控制面：Hostname Resolution 核心逻辑 | `hostname_resolution.rs` (新增) | +306 | 计算 route × listener hostname 交集 |
| T3 | 控制面：HTTPRoute/GRPCRoute handler 集成 | `handlers/{http,grpc}_route.rs` | +58 | parse 阶段调用 hostname resolution |
| T4 | 控制面：Gateway → Route 双向索引 | `gateway_route_index.rs` (新增), `handlers/mod.rs`, `mod.rs` | +312 | Gateway 变化时 requeue 受影响 route |
| T5 | 控制面：Gateway handler requeue 逻辑 | `handlers/gateway.rs` | +40 | Gateway listener hostname 变化时触发 route requeue |
| T6 | 数据面：HTTP Route hostname 解析简化 | `http_routes/conf_handler_impl.rs`, `tests.rs` | +215/-243 | 用 `get_effective_hostnames` 替代旧函数链 |
| T7 | 数据面：gRPC Route hostname 匹配增强 | `grpc_routes/{match_unit,conf_handler_impl,match_engine}.rs` | +154/-126 | gRPC route 增加 effective_hostnames 校验 |
| T8 | 数据面：HTTP Listener Isolation | `config_store.rs`, `route_match.rs` | +251 | `has_more_specific_listener` 按端口隔离 |
| T9 | 数据面：去除冗余 route rebuild | `gateway/handler.rs` | +21/-21 | 移除 `rebuild_from_stored_routes` 调用 |
| T10 | 测试修复：HeaderCertAuth 运行时证书 | `header_cert_auth.rs`, `run_integration.sh` | +16/-3 | 运行时读取生成的 TLS 证书 |

---

## T1. 类型层：新增 `resolved_hostnames` 字段

### 文件
- `src/types/resources/http_route.rs`（+5 行）
- `src/types/resources/grpc_route.rs`（+5 行）

### 改动
在 `HTTPRouteSpec` 和 `GRPCRouteSpec` 中新增：
```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub resolved_hostnames: Option<Vec<String>>,
```

### Review 要点
1. 字段位于 `hostnames` 之后、`rules` 之前，位置合理
2. 使用 `skip_serializing_if` 确保不影响现有序列化（无 `resolved_hostnames` 时不输出）
3. 控制面写入，数据面读取，中间通过 gRPC `ConfSync` 同步

### ⚠️ 需确认
- [ ] `resolved_hostnames` 字段经过 gRPC ConfSync 传输后，数据面确实能收到此字段（与 protobuf/JSON 序列化兼容）

---

## T2. 控制面：Hostname Resolution 核心逻辑

### 文件
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/hostname_resolution.rs`（新增，306 行）

### 核心结构
```rust
pub struct ResolvedHostnames {
    pub hostnames: Vec<String>,
    pub annotation: Option<String>,  // e.g. "intersected;inherited:foo.com"
}
```

### 核心函数

**`resolve_effective_hostnames`** — 主入口，遍历 parentRefs，对每个 Gateway：

| Route hostnames | Listener hostname | 行为 | annotation |
|----------------|-------------------|------|------------|
| 有 | 有 | 计算交集 | `intersected` |
| 有 | 无（catch-all） | passthrough | `passthrough` |
| 无 | 有 | 继承 listener hostname | `inherited:{hostname}` |
| 无 | 无 | catch-all `"*"` | `catch-all` |
| 任意 | Gateway 不存在 | fallback 到 route 原始 hostnames | `gateway-pending` |

**`collect_listener_hostnames`** — 根据 parentRef 的 sectionName/port 过滤匹配的 listener

**`compute_hostname_intersection`** — 按 Gateway API 规范计算交集：

| Listener | Route | Result |
|----------|-------|--------|
| `example.com` | `example.com` | `example.com` |
| `*.wildcard.io` | `foo.wildcard.io` | `foo.wildcard.io` |
| `very.specific.com` | `*.specific.com` | `very.specific.com` |
| `*.bar.com` | `*.foo.bar.com` | `*.foo.bar.com` |
| `foo.com` | `bar.com` | None |

### Review 要点
1. 使用 `HashSet<String>` (`seen`) 去重，避免多 parentRef 指向同 Gateway 时重复 hostname
2. `actions` 数组用于生成 `edgion.io/hostname-resolution` annotation，便于调试和 access log
3. 当 `all_effective` 为空时 fallback 到 `"*"`，防止 route 完全不可达（第 110-112 行）
4. 11 个单元测试覆盖 `compute_hostname_intersection` 各场景

### ⚠️ 需确认
- [ ] `lookup_gateway` 函数在 `parse` 阶段调用时 Gateway 是否一定已就绪？如果不是，`gateway-pending` fallback 是否会在后续 Gateway arrive 时被 requeue 覆盖？
- [ ] annotation 值格式（分号分隔的 action 列表）是否需要文档化

---

## T3. 控制面：HTTPRoute/GRPCRoute handler 集成

### 文件
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`（+29 行）
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`（+29 行）

### 改动
三处变更（HTTP/gRPC 完全对称）：

**1. `parse()` 中新增 hostname resolution 调用**（`register_service_refs` 之后、`ref_denied` 标记之前）：
```rust
if let Some(parent_refs) = &route.spec.parent_refs {
    let resolved = super::hostname_resolution::resolve_effective_hostnames(
        route.spec.hostnames.as_ref(),
        parent_refs,
        route_ns_resolve,
    );
    route.spec.resolved_hostnames = if resolved.hostnames.is_empty() {
        None
    } else {
        Some(resolved.hostnames)
    };
    if let Some(annotation) = resolved.annotation {
        let annotations = route.metadata.annotations.get_or_insert_with(Default::default);
        annotations.insert("edgion.io/hostname-resolution".to_string(), annotation);
    }
}
```

**2. `on_change()` 中新增** `update_gateway_route_index` 调用

**3. `on_delete()` 中新增** `remove_from_gateway_route_index` 调用

### Review 要点
1. HTTP 和 gRPC 的逻辑完全对称，共享 `hostname_resolution` 模块
2. annotation 写入为覆盖式 (`insert`)，每次 parse 更新最新值
3. `on_change`/`on_delete` 中的 index 操作确保 Gateway requeue 机制正常工作

---

## T4. 控制面：Gateway → Route 双向索引

### 文件
- `src/core/conf_mgr/sync_runtime/resource_processor/gateway_route_index.rs`（新增，287 行）
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs`（+21 行）
- `src/core/conf_mgr/sync_runtime/resource_processor/mod.rs`（+4 行）

### 核心结构
```rust
pub struct GatewayRouteIndex {
    forward: RwLock<HashMap<String, HashSet<RouteEntry>>>,    // gateway_key → routes
    reverse: RwLock<HashMap<RouteEntry, HashSet<String>>>,    // route → gateway_keys
    gateway_hostnames: RwLock<HashMap<String, Vec<String>>>,  // hostname 变化检测缓存
}
```

### 核心方法
| 方法 | 调用方 | 说明 |
|------|--------|------|
| `update_route(kind, key, parent_refs, ns)` | route `on_change` | 从 parentRefs 提取 Gateway key，更新双向索引 |
| `remove_route(kind, key)` | route `on_delete` | 删除 route 时清理索引 |
| `get_routes_for_gateway(gw_key)` | gateway `on_change` | Gateway 变化时获取受影响 route 列表 |
| `update_gateway_hostnames(gw_key, hostnames)` | gateway `on_change` | 排序后比较判断 hostname 是否变化 |
| `remove_gateway_hostnames(gw_key)` | gateway `on_delete` | 清理 hostname 缓存 |

### 辅助函数（handlers/mod.rs）
```rust
pub(crate) fn update_gateway_route_index(...)  // route on_change 调用
pub(crate) fn remove_from_gateway_route_index(...)  // route on_delete 调用
```

### Review 要点
1. 使用 ALL parentRefs（不仅 accepted），因为 pending route 也需要在 Gateway 变化时 requeue
2. 只过滤 `group=gateway.networking.k8s.io && kind=Gateway` 的 parentRef，跳过 Service 等
3. hostname 缓存使用排序后的 `Vec` 比较，避免因顺序差异触发不必要的 requeue
4. 锁粒度：`forward` 和 `reverse` 分别用 `RwLock`，`update_route` 同时写两把锁
5. 6 个单元测试覆盖：基本注册、multi-gateway、update-replace、remove、non-gateway-ref 过滤

### ⚠️ 需确认
- [ ] `update_route` 中同时获取 `forward` 和 `reverse` 两把写锁，是否存在死锁风险？（当前总是以固定顺序获取，应无问题）
- [ ] `RouteEntry` 只有 `kind` 和 `key`，不含 `sectionName` — 是否需要区分同一 route 对同一 Gateway 的不同 sectionName 绑定？

---

## T5. 控制面：Gateway handler requeue 逻辑

### 文件
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`（+40 行）

### `on_change()` 新增逻辑（~32 行）
```
1. 提取当前 Gateway 的所有 listener hostnames
2. route_index.update_gateway_hostnames() → 判断是否变化
3. 仅在 hostname 变化时 → 获取引用此 Gateway 的 route → ctx.requeue()
```

### `on_delete()` 新增
```rust
get_gateway_route_index().remove_gateway_hostnames(&gateway_key);
```

### Review 要点
1. **循环安全**：route `on_change` 只在 parentRef attachment 变化时 requeue Gateway；hostname-only 变化不反向触发 Gateway requeue
2. hostname 变化检测通过缓存实现，大多数 Gateway update 不触发 route requeue
3. 日志记录了 requeue 的 route 数量和 gateway key，便于排查

---

## T6. 数据面：HTTP Route hostname 解析简化

### 文件
- `src/core/routes/http_routes/conf_handler_impl.rs`（+202, -243 行，净减 ~80 行）
- `src/core/routes/http_routes/tests.rs`（+13 行）

### 删除的函数
| 函数 | 行数 | 说明 |
|------|------|------|
| `resolve_effective_hostnames_for_route()` | ~30 | 需要查询 GatewayStore 的旧逻辑 |
| `resolve_all_effective_hostnames()` | ~20 | 遍历 parentRefs 收集 hostname |
| `RouteManager::rebuild_from_stored_routes()` | ~15 | Gateway 变化时重建全部 route table |

### 新增的函数
```rust
fn get_effective_hostnames(route: &HTTPRoute) -> Vec<String> {
    // 优先级：resolved_hostnames > spec.hostnames > CATCH_ALL_HOSTNAME ("*")
}
```

### 调用点替换（7 处）
- `resolve_all_effective_hostnames(route, ns, refs)` → `get_effective_hostnames(route)`
- 涉及 `full_set`、`partial_update`、`parse_http_routes_to_domain_rules`、`rebuild_wildcard_engine`

### 测试更新
- `test_route_inherits_gateway_listener_hostname`：修改为设置 `route.spec.resolved_hostnames` 模拟控制面行为
- 新增 5 个 `get_effective_hostnames` 单元测试：
  - `test_get_effective_hostnames_prefers_resolved`
  - `test_get_effective_hostnames_falls_back_to_spec_hostnames`
  - `test_get_effective_hostnames_catch_all_when_empty`
  - `test_get_effective_hostnames_skips_empty_resolved`
  - `test_get_effective_hostnames_multiple_resolved`

### Review 要点
1. 大幅简化：不再需要在数据面查询 `GatewayStore`
2. 移除 `use crate::core::gateway::gateway::get_global_gateway_store` import
3. `rebuild_from_stored_routes` 的删除是安全的——控制面通过 requeue 机制重新解析 route

---

## T7. 数据面：gRPC Route hostname 匹配增强

### 文件
- `src/core/routes/grpc_routes/match_unit.rs`（+154, -126 行）
- `src/core/routes/grpc_routes/conf_handler_impl.rs`（+18 行）
- `src/core/routes/grpc_routes/match_engine.rs`（+10 行）

### 问题背景
gRPC route 使用 `GrpcMatchEngine` 按 **service/method** 分桶，**不按 hostname 分桶**（`DomainGrpcRouteRules` 注释："no hostname-based separation"）。如果没有显式 hostname 检查，catch-all listener 上的 route 会匹配任何 hostname 的请求，导致负面测试失败。

### `GrpcRouteInfo` 结构变更
```rust
// 旧
pub struct GrpcRouteInfo {
    pub parent_refs: Option<Vec<ParentReference>>,
    pub hostnames: Option<Vec<String>>,
}

// 新
pub struct GrpcRouteInfo {
    pub parent_refs: Option<Vec<ParentReference>>,
    pub effective_hostnames: Vec<String>,  // 从 resolved_hostnames 或 hostnames 取得
}
```

### `deep_match` 新增 hostname 检查
在 `check_gateway_listener_match` 之前，新增：
```rust
if !self.matches_hostname(hostname) {
    return Ok(None);
}
```

### `matches_hostname` 方法
```rust
fn matches_hostname(&self, request_hostname: &str) -> bool {
    // catch-all (["*"]) → always match
    // exact: case-insensitive comparison
    // wildcard: reuse hostname_matches_listener from route_match
}
```

### 删除的代码
| 代码 | 说明 |
|------|------|
| `match_hostname()` | 旧的 hostname 列表匹配函数 |
| `hostname_matches()` | 旧的单个 hostname 通配符匹配函数 |
| 4 个 `#[cfg(test)]` 测试 | 旧的 hostname 匹配测试 |

### `conf_handler_impl.rs` 新增
```rust
fn get_effective_hostnames(route: &GRPCRoute) -> Vec<String> {
    // 与 HTTP 版本逻辑完全一致：resolved_hostnames > hostnames > CATCH_ALL_HOSTNAME
}
```

### Review 要点
1. **核心修复**：`grpc_hostname_match_negative` 和 `grpc_section_name_mismatch` 测试现在正确通过
2. 新的 `matches_hostname` 复用 `route_match::hostname_matches_listener`，确保 HTTP/gRPC 匹配语义一致
3. `CATCH_ALL_HOSTNAME` 常量定义在 `match_unit.rs`，由 `conf_handler_impl.rs` 引用
4. 旧的 `hostname_matches` 只支持单级通配符，新实现通过 `hostname_matches_listener` 支持多级

---

## T8. 数据面：HTTP Listener Isolation（含跨端口修复）

### 文件
- `src/core/gateway/gateway/config_store.rs`（+230 行）
- `src/core/gateway/gateway/route_match.rs`（+21 行）

### 问题背景
HTTP Listener Isolation 要求：同一 Gateway+Port 上如果有更具体的 listener hostname 匹配请求，则更泛化的 listener 不应处理该请求。**旧实现不区分端口**，导致不同端口的 listener 互相干扰（例如 HTTP 80 的 catch-all 被 HTTPS 443 的 `*.example.com` 拦截）。

### `ListenerConfig` 新增 `port` 字段
```rust
pub struct ListenerConfig {
    pub name: String,
    pub port: i32,  // ← 新增
    pub hostname: Option<String>,
    pub allowed_routes: Option<AllowedRoutes>,
}
```

### `has_more_specific_listener` 方法
```rust
pub fn has_more_specific_listener(
    &self,
    hostname: &str,
    current_listener_hostname: Option<&str>,
    current_port: i32,  // ← 新增参数
) -> bool
```

核心逻辑：
1. 精确 hostname listener → 立即 return false（它自己就是最具体的）
2. 遍历 `listener_map`，**只考虑同端口** (`config.port == current_port`)
3. 对于 catch-all (None)：任何同端口带 hostname 的 listener 匹配请求 → blocked
4. 对于 wildcard (`*.foo.com`)：同端口有精确匹配或更长通配符 → blocked

### `route_match.rs` 调用更新
```rust
// 在 check_gateway_listener_match 中
let gateways = config_store.load_gateways();  // 循环外一次性加载

// 循环内
let gw_key = gi.gateway_key();
if let Some(gw_config) = gateways.get(&gw_key) {
    if gw_config.has_more_specific_listener(
        request_hostname,
        config.hostname.as_deref(),
        config.port,
    ) {
        continue;
    }
}
```

### 新增 `load_gateways` 方法
```rust
pub fn load_gateways(&self) -> arc_swap::Guard<Arc<HashMap<...>>>
```
避免循环内重复 `load()`。

### 测试（10 个）
| 测试 | 场景 |
|------|------|
| `catchall_vs_exact` | Catch-all 被精确 listener 阻断 |
| `catchall_vs_wildcard` | Catch-all 被通配符 listener 阻断 |
| `catchall_no_others` | 单独 catch-all 不被阻断 |
| `wildcard_vs_exact` | 通配符被精确 listener 阻断 |
| `wildcard_vs_more_specific_wildcard` | `*.example.com` 被 `*.foo.example.com` 阻断 |
| `wildcard_no_more_specific` | 单独通配符不被阻断 |
| `exact_always_false` | 精确 listener 永不被阻断 |
| `full_isolation` | 完整 4-listener 场景（对应 conformance test） |
| `cross_port_independent` | **跨端口独立性**：port 80 catch-all 不受 port 443 wildcard 影响 |
| `store_update_and_remove` | 配置增删改 |

### Review 要点
1. **核心修复**：HTTPRoute_Match 全部 10 个测试通过
2. 遍历 `listener_map` 而非查找 `host_map`/`wildcard_host_map`，正确性优先（listener 数量通常个位数）
3. `host_map`/`wildcard_host_map` 仍保留（`has_host` 测试方法使用），但 `has_more_specific_listener` 不依赖

### ⚠️ 需确认
- [ ] `host_map`/`wildcard_host_map` 是否还有生产使用场景？如仅用于测试，可考虑清理

---

## T9. 数据面：Gateway handler 去除冗余 route rebuild

### 文件
- `src/core/gateway/gateway/handler.rs`（+21, -21 行）

### 删除的代码
| 代码 | 说明 |
|------|------|
| `let route_manager = get_global_route_manager()` | full_set 和 partial_update 中各一处 |
| `route_manager.rebuild_from_stored_routes()` | full_set 中调用 |
| `need_route_rebuild` 标志和相关逻辑 | partial_update 中 |
| `drop(store)` 释放锁后 rebuild | partial_update 末尾 |
| `use crate::core::routes::http_routes::get_global_route_manager` | import |

### 新增注释
```rust
// Hostname resolution is handled by the controller via resolved_hostnames.
// Gateway changes trigger route requeue at the controller level.
```

### Review 要点
1. 之前 Gateway 变化需在数据面 rebuild 所有 route（hostname 继承逻辑在数据面），现在移到控制面
2. 删除的 `drop(store)` 之前用于释放 gateway store 写锁以避免死锁——现在不再需要

---

## T10. 测试修复：HeaderCertAuth 运行时证书

### 文件
- `examples/code/client/suites/edgion_plugins/header_cert_auth/header_cert_auth.rs`（+17, -3 行）
- `examples/test/scripts/integration/run_integration.sh`（+2 行）

### 问题背景
测试用 `include_str!` 在编译时嵌入 `ClientCert_edge_backend-client-cert.yaml` 模板文件，但该模板是 `data: {}`（空）。实际 TLS 证书由 `generate_mtls_certs.sh` 在运行时生成到 `$WORK_DIR/generated-secrets/` 目录。

### 修复方案
**`header_cert_auth.rs`：**
```rust
// 新增：运行时证书路径
const RUNTIME_SECRET_RELATIVE: &str =
    "generated-secrets/HTTPRoute/Backend/BackendTLS/ClientCert_edge_backend-client-cert.yaml";

fn load_runtime_secret_yaml() -> Option<String> {
    let work_dir = std::env::var("EDGION_WORK_DIR").ok()?;
    let path = std::path::Path::new(&work_dir).join(RUNTIME_SECRET_RELATIVE);
    std::fs::read_to_string(&path).ok()
}

fn load_client_cert_pem() -> Result<String, String> {
    // 优先运行时生成的文件，fallback 到编译时模板
    let yaml = load_runtime_secret_yaml()
        .unwrap_or_else(|| COMPILE_TIME_SECRET_YAML.to_string());
    // ...
}
```

**`run_integration.sh`：**
在两处设置环境变量的位置新增：
```bash
export EDGION_WORK_DIR="${WORK_DIR}"
```

### Review 要点
1. 保留编译时模板作为 fallback（模板文件中将来可能预填充测试证书）
2. 运行时路径通过 `EDGION_WORK_DIR` 环境变量获取，与 `start_all_with_conf.sh` 中的 export 一致
3. 错误信息从 `"missing tls.crt in test fixture"` 改为 `"missing tls.crt in test fixture (runtime and compile-time)"`

---

## 集成测试结果

| 测试 | 修复前 | 修复后 | 对应 Task |
|------|-------|-------|-----------|
| HTTPRoute_Match (10 tests) | ❌ 全部 404 | ✅ 10/10 | T8 |
| GRPCRoute_Match (5 tests) | ❌ 3/5 失败 | ✅ 5/5 | T7 + T8 |
| EdgionPlugins_HeaderCertAuth (3 tests) | ❌ 2/3 失败 | ✅ 3/3 | T10 |
| **总计** | **56/59 (94%)** | **59/59 (100%)** | |

---

## 文件改动清单

### 修改文件（17 个）

| 文件 | Task | +/- |
|------|------|-----|
| `src/types/resources/http_route.rs` | T1 | +5 |
| `src/types/resources/grpc_route.rs` | T1 | +5 |
| `src/core/conf_mgr/.../handlers/http_route.rs` | T3 | +29 |
| `src/core/conf_mgr/.../handlers/grpc_route.rs` | T3 | +29 |
| `src/core/conf_mgr/.../handlers/mod.rs` | T4 | +21 |
| `src/core/conf_mgr/.../mod.rs` | T4 | +4 |
| `src/core/conf_mgr/.../handlers/gateway.rs` | T5 | +40 |
| `src/core/routes/http_routes/conf_handler_impl.rs` | T6 | +202/-243 |
| `src/core/routes/http_routes/tests.rs` | T6 | +13 |
| `src/core/routes/grpc_routes/match_unit.rs` | T7 | +154/-126 |
| `src/core/routes/grpc_routes/conf_handler_impl.rs` | T7 | +18 |
| `src/core/routes/grpc_routes/match_engine.rs` | T7 | +10 |
| `src/core/gateway/gateway/config_store.rs` | T8 | +230 |
| `src/core/gateway/gateway/route_match.rs` | T8 | +21 |
| `src/core/gateway/gateway/handler.rs` | T9 | +21/-21 |
| `examples/code/client/.../header_cert_auth.rs` | T10 | +17/-3 |
| `examples/test/scripts/.../run_integration.sh` | T10 | +2 |

### 新增文件（2 个）

| 文件 | Task | 行数 |
|------|------|-----|
| `src/core/conf_mgr/.../gateway_route_index.rs` | T4 | 287 |
| `src/core/conf_mgr/.../handlers/hostname_resolution.rs` | T2 | 306 |

---

## 总结

本次重构将 hostname resolution 逻辑从数据面迁移到控制面，核心改动包括：

1. **控制面**（T2-T5）：在 route parse 阶段计算 effective hostnames（route × listener 交集），存入 `resolved_hostnames` 字段；建立 Gateway → Route 双向索引确保 Gateway 变化触发 route 重新解析
2. **数据面**（T6-T9）：简化 HTTP route 的 hostname 解析（直接使用 `resolved_hostnames`）；为 gRPC route 补充 hostname 校验（弥补不按 hostname 分桶的架构缺陷）；实现正确的 HTTP Listener Isolation（按端口隔离）
3. **测试修复**（T10）：HeaderCertAuth 测试从运行时生成的证书文件读取 TLS 证书
