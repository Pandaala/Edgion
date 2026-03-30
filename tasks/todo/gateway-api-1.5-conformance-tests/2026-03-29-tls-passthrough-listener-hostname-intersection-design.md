# TLS Passthrough Listener Hostname Intersection — 设计文档

**日期**: 2026-03-29
**状态**: 待实现
**关联测试**: `TLSRouteHostnameIntersection` (Gateway API v1.5 conformance)

---

## 问题陈述

`TLSRouteHostnameIntersection` conformance test 包含 4 个 Gateway（均在 port 443，TLS Passthrough），
分别有不同的 listener hostname（`abc.example.com`、`*.example.com`、`*.com`、空）。
所有 TLSRoute 的 `resolved_listeners` 经去重后都指向同一个 `ListenerKey("0.0.0.0:443/TLS")`，
导致所有路由混入一张全局 route table。

**两个直接 bug**：

1. **rejection 失败**：`non.matching.com` 通过全局表中的 `*.com` 路由被匹配（`HashHost` 用 suffix
   方式：`non.matching.com` 以 `.com` 结尾），使得期望 TCP 拒绝的子测试全部失败。

2. **effective hostname 缺失**：TLSRoute-2.2（route hostname=`*.com`，GW2 listener=`*.example.com`）
   的 effective hostname 应为 `*.example.com`（两者的交集），但 route table 使用原始 `*.com` 建索引。
   `other.example.com` 无法匹配 `*.com`（在 Gateway API 单标签语义下），导致 subtest 2b 路由到
   错误后端或失败。

**根因**：`resolve_listener_keys()` 只按 port + namespace 过滤，完全没有检查 listener hostname
与 route hostname 的交集；`resolved_listeners` 只携带 `ListenerKey`，丢失了 listener hostname 信息；
`TlsRouteTable` 使用原始 route hostname 建索引，而非 effective hostname。

---

## 设计目标

- 实现 Gateway API spec 要求的 per-listener hostname scoping：连接到达时先用 SNI 选择最匹配的
  listener 上下文，再在该上下文的 route table 中匹配 route。
- `TlsRouteTable` 使用 effective hostnames（listener ∩ route）建索引。
- 通过 `TLSRouteHostnameIntersection` 所有子测试（`--tls-only` 模式）。
- 新增本地集成测试 `TLSRoute/ListenerHostnameIntersection` 覆盖完整行为。

---

## 方案 A：SNI 优先 listener 选择 + 按 listener hostname 隔离 route table

### 1. 新类型 `ResolvedListener`

**文件**: `src/types/resolved_listener.rs`（新建）

```rust
/// Per-(route, listener) 的解析结果，由 Controller 预计算后存入路由 spec。
///
/// 包含物理 listener key、listener hostname，以及 route hostname 与 listener
/// hostname 的交集（effective hostnames）。Gateway 使用这些信息建立隔离的
/// per-listener-hostname route table，并在连接时基于 SNI 选择正确的 table。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedListener {
    /// 物理 listener 标识（地址 + 协议），如 "0.0.0.0:443/TLS"
    pub key: ListenerKey,
    /// Listener 的 hostname。None = 匹配所有 SNI（空 hostname）
    pub hostname: Option<String>,
    /// route.spec.hostnames ∩ listener.hostname 的结果。
    /// 用于 route table 建索引，确保只有 effective hostname 范围内的 SNI 才会匹配此路由。
    pub effective_hostnames: Vec<String>,
}
```

同时在 `src/types/mod.rs` 中 pub re-export。

### 2. Controller 改动（`route_utils.rs`）

#### 新工具函数：`compute_effective_hostnames`

```rust
/// 计算 route hostname 列表与 listener hostname 的交集（effective hostnames）。
///
/// 交集规则（per Gateway API spec）：
/// - listener hostname 为 None（空）：effective = route hostnames 原样保留
/// - 精确 vs 精确：相等时 effective = [hostname]
/// - 通配 listener vs 精确 route：route 落在 listener 通配域内 → effective = [route]
/// - 精确 listener vs 通配 route：listener 落在 route 通配域内 → effective = [listener]
/// - 通配 vs 通配：取更具体者（较长 suffix）作为 effective
pub fn compute_effective_hostnames(
    listener_hn: Option<&str>,
    route_hostnames: &[String],
) -> Vec<String>
```

实现遍历 `route_hostnames`，对每个 `route_hn` 调用已有的 `hostnames_intersect(listener_hn, route_hn)`。
若有交集，取两者中更具体的一方（更长 suffix 的通配，或精确）作为 effective hostname 加入结果。

#### 改造 `resolve_listener_keys()` → `resolve_listeners()`

```rust
pub fn resolve_listeners(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    fallback_namespace: Option<&str>,
    route_hostnames: &[String],        // 新增参数
) -> Vec<ResolvedListener>
```

在原有 port + namespace 过滤基础上，增加：
```rust
// 新增：hostname intersection 检查
let effective = compute_effective_hostnames(
    listener.hostname.as_deref(),
    route_hostnames,
);
// 若 route_hostnames 非空但 effective 为空，说明无交集，跳过此 listener
if !route_hostnames.is_empty() && effective.is_empty() {
    continue;
}
listener_resolved.push(ResolvedListener {
    key: ListenerKey::new(addr, &listener.protocol),
    hostname: listener.hostname.clone(),
    effective_hostnames: effective,
});
```

去重逻辑：按 `(key, hostname)` 去重（原先只按 `key` 去重）。

#### 调用方（TLSRoute handler）

在 `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs` 中，
将 `resolve_listener_keys(...)` 替换为 `resolve_listeners(..., &route_hostnames)`，
并将结果存入 `spec.resolved_listeners`。

### 3. `TLSRouteSpec` 类型变更

**文件**: `src/types/resources/tls_route.rs`

```rust
// 之前
pub resolved_listeners: Option<Vec<ListenerKey>>,

// 之后
pub resolved_listeners: Option<Vec<ResolvedListener>>,
```

所有使用 `resolved_listeners` 的地方均需同步更新（主要是 `routes_mgr.rs`）。

### 4. Gateway 路由管理改动（`routes_mgr.rs`）

#### `GlobalTlsRouteManagers`：新 key 类型

```rust
// 之前
by_listener: DashMap<ListenerKey, Arc<TlsRouteManager>>,

// 之后
by_listener: DashMap<(ListenerKey, Option<String>), Arc<TlsRouteManager>>,
//                    ^-- 物理端口   ^-- listener hostname
```

#### `bucket_routes_by_listener()` 改造

按 `(key, hostname)` 分桶，同时携带 effective_hostnames：

```rust
fn bucket_routes_by_listener(
    &self,
) -> HashMap<(ListenerKey, Option<String>), HashMap<String, (Arc<TLSRoute>, Vec<String>)>>
//                                                           route_key  route  effective_hostnames
```

遍历 `route.spec.resolved_listeners`，对每条 `ResolvedListener` 按 `(key, hostname)` 分桶，
value 为 `(route.clone(), resolved.effective_hostnames.clone())`。

#### `TlsRouteManager::rebuild()` 签名变更

```rust
pub fn rebuild(&self, routes: &HashMap<String, (Arc<TLSRoute>, Vec<String>)>)
```

将 `(route, effective_hostnames)` 传给 `TlsRouteTable::from_routes_with_effective_hostnames()`。

#### 新方法 `find_best_manager_for_sni()`

```rust
/// 根据 SNI 在指定物理端口下选择最匹配的 listener hostname 对应的 manager。
///
/// 选择优先级（per Gateway API spec listener hostname specificity）：
/// 1. 精确匹配（listener_hostname == sni）
/// 2. 最长通配 suffix（".example.com" 优于 ".com"）
/// 3. 空 hostname（匹配所有）
///
/// 如果没有任何 listener hostname 能 cover 该 SNI，返回 None（连接被拒绝）。
pub fn find_best_manager_for_sni(
    &self,
    sni: &str,
    port_key: &ListenerKey,
) -> Option<Arc<TlsRouteManager>>
```

实现：遍历 `by_listener` 中所有 key 匹配 `port_key` 的条目，用
`sni_matches_listener_hostname(sni, listener_hn)` 过滤，再按 specificity 排序取第一个。

内部工具函数：
```rust
fn sni_matches_listener_hostname(sni: &str, listener_hn: Option<&str>) -> bool {
    match listener_hn {
        None => true,
        Some(hn) => {
            if hn == sni { return true; }
            if let Some(suffix) = hn.strip_prefix("*.") {
                sni.ends_with(&format!(".{}", suffix))
            } else {
                false
            }
        }
    }
}

fn listener_hostname_specificity(hn: &Option<String>) -> usize {
    match hn {
        None => 0,
        Some(h) => h.len(),  // 越长越具体
    }
}
```

### 5. `TlsRouteTable` 改动（`gateway_tls_routes.rs`）

新方法，用 effective hostnames 代替原始 route hostnames 建索引：

```rust
pub fn from_routes_with_effective_hostnames(
    routes: &HashMap<String, (Arc<TLSRoute>, Vec<String>)>,
) -> Self
```

实现与 `from_routes()` 基本相同，只是把 `route.spec.hostnames` 换成传入的 `effective_hostnames`。

保留 `from_routes()` 用于不需要 effective hostname 的场景（如 TLS terminate 模式或现有单 Gateway 场景）。

### 6. `EdgionTlsPassthroughProxy` 改动（`passthrough_proxy.rs`）

移除字段 `tls_route_manager: Arc<TlsRouteManager>`，在 `handle_connection()` 中改为运行时查找：

```rust
async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni: &str, initial_bytes: &[u8]) {
    // 新：SNI 优先 listener 选择
    let manager = match get_global_tls_route_managers()
        .find_best_manager_for_sni(sni, &self.listener_key)
    {
        Some(m) => m,
        None => {
            ctx.err_log = Some("no matching listener for SNI".to_string());
            return;
        }
    };

    let route_table = manager.load_route_table();
    let tls_route = match route_table.match_route(sni) {
        Some(route) => route,
        None => {
            ctx.err_log = Some("no matching route".to_string());
            return;
        }
    };
    // ... 后续不变
}
```

在 `base.rs` 中，被 `bound_listeners` 去重跳过的 listener，仍需注册其 `(ListenerKey, hostname)` 到
`GlobalTlsRouteManagers`，以便路由更新时能建立对应 manager（由 `rebuild_all_listener_managers`
的分桶逻辑自动处理，无需额外显式注册）。

---

## 集成测试设计

### 新增 Suite：`TLSRoute/ListenerHostnameIntersection`

**端口**（新增到 `ports.json`）：
```json
"TLSRoute/ListenerHostnameIntersection": { "tls": 31291 }
```

### YAML 资源（`examples/test/conf/TLSRoute/ListenerHostnameIntersection/`）

**01_Gateways.yaml** — 3 个 Gateway，同一端口 31291，不同 listener hostname（由具体到宽泛）：

```yaml
# gw-lhi-narrow: 最具体，精确通配
kind: Gateway
name: gw-lhi-narrow
listeners:
  - name: narrow
    protocol: TLS
    port: 31291
    hostname: "*.narrow.example.com"
    tls: { mode: Passthrough }
---
# gw-lhi-middle: 中等通配
kind: Gateway
name: gw-lhi-middle
listeners:
  - name: middle
    protocol: TLS
    port: 31291
    hostname: "*.example.com"
    tls: { mode: Passthrough }
---
# gw-lhi-broad: 空 hostname，匹配所有
kind: Gateway
name: gw-lhi-broad
listeners:
  - name: broad
    protocol: TLS
    port: 31291
    tls: { mode: Passthrough }
```

**03_TLSRoutes.yaml** — 3 个 TLSRoute（每个 Gateway 一条路由）：

| 名称 | parent Gateway | route hostname | backend | effective hostname |
|------|---------------|---------------|---------|-------------------|
| `route-lhi-narrow` | gw-lhi-narrow | `test.narrow.example.com` | test-https-backend | `test.narrow.example.com` |
| `route-lhi-middle` | gw-lhi-middle | `*.com` | test-https-backend | `*.example.com`（intersection） |
| `route-lhi-broad` | gw-lhi-broad | `*.com` | test-https-backend | `*.com` |

所有路由共用同一 backend（现有 `test-https-backend` Service，port 30051）。
测试通过 TLS 握手是否成功来区分"连通"和"拒绝"——gateway 在无匹配 route 时立即关闭 TCP 连接，
TLS 握手自然失败。由于 `make_tls_connector()` 跳过证书校验，SNI 域名无需有效证书。

**Service_test-https-backend.yaml / EndpointSlice_test-https-backend.yaml**
— 复用已有 `TLSRoute/Passthrough/` 中的同名文件（软链接或直接复制）。

### 测试矩阵

| SNI | 匹配 listener（优先级）| route table 结果 | 期望 |
|-----|----------------------|----------------|------|
| `test.narrow.example.com` | GW-narrow（精确+最长 suffix） | R-narrow 精确匹配 | CONNECTED |
| `other.narrow.example.com` | GW-narrow（比 middle+broad 更具体）| R-narrow 不匹配（仅精确 route），无 fallback | **REJECTED** |
| `foo.narrow.example.com` | GW-narrow（同上）| R-narrow 不匹配，无 fallback | **REJECTED** |
| `foo.example.com` | GW-middle（比 broad 更具体） | R-middle effective `*.example.com` 匹配 | CONNECTED |
| `bar.example.com` | GW-middle | 同上 | CONNECTED |
| `foo.other.com` | GW-broad（empty，narrow/middle 均不匹配）| R-broad `*.com` suffix 匹配 | CONNECTED |
| `x.com` | GW-broad（empty）| R-broad `*.com` 匹配 | CONNECTED |
| `foo.something.org` | GW-broad（empty）| R-broad `*.com` 不匹配（`.org`） | **REJECTED** |

### 测试用例（`examples/code/client/suites/tls_route/listener_hostname_intersection/`）

**Suite 名称**：`TlsListenerHostnameIntersectionTestSuite`，8 个 test case：

| # | 名称 | SNI | 期望 | 覆盖的场景 |
|---|------|-----|------|-----------|
| 1 | `narrow_exact_route_connected` | `test.narrow.example.com` | CONNECTED | GW-narrow 精确路由 |
| 2 | `narrow_listener_no_wildcard_route_rejected` | `other.narrow.example.com` | **REJECTED** | 关键：GW-narrow 被选中但无通配 route，不 fallback 到 GW-middle/broad |
| 3 | `narrow_listener_another_wildcard_sni_rejected` | `foo.narrow.example.com` | **REJECTED** | 同上，换一个 SNI 确认一致性 |
| 4 | `middle_listener_effective_hostname_connected` | `foo.example.com` | CONNECTED | effective hostname：R-middle raw `*.com` 经 effective 变为 `*.example.com` |
| 5 | `middle_listener_another_sni_connected` | `bar.example.com` | CONNECTED | 同上 |
| 6 | `broad_listener_routes_unrelated_domain` | `foo.other.com` | CONNECTED | GW-broad empty listener + `*.com` route |
| 7 | `broad_listener_routes_simple_com` | `x.com` | CONNECTED | GW-broad，`*.com` 匹配单标签 |
| 8 | `non_com_sni_always_rejected` | `foo.something.org` | **REJECTED** | 无 listener 匹配 `.org` 域名 |

**关键验证点**：
- 用例 2/3：验证"选中 listener 内无匹配 route → 拒绝，不 fallback"（修复前：`*.com` 路由会意外匹配）
- 用例 4/5：验证 effective hostname 替代 raw hostname 建索引（修复前：`*.com` 不匹配 `*.example.com` 域名）
- 用例 6/7 与 2/3 对比：相同 `*.com` route 在 GW-broad 上正常工作

### 注册到 test_client.rs

以下三处需补全：
- `resolve_suite("TLSRoute", "ListenerHostnameIntersection")` → 映射到新 suite
- `suite_to_port_key("TLSRoute/ListenerHostnameIntersection")` → `"tls": 31291`
- `add_suites_for_suite()` → 添加 `TlsListenerHostnameIntersectionTestSuite`

---

## 受影响文件列表

| 文件 | 改动类型 |
|------|---------|
| `src/types/resolved_listener.rs` | 新建 |
| `src/types/mod.rs` | 添加 re-export |
| `src/types/resources/tls_route.rs` | `resolved_listeners` 类型变更 |
| `src/core/controller/.../route_utils.rs` | `resolve_listener_keys` → `resolve_listeners`，新增 `compute_effective_hostnames` |
| `src/core/controller/.../tls_route.rs` | 调用 `resolve_listeners` |
| `src/core/gateway/routes/tls/routes_mgr.rs` | key 类型、分桶逻辑、新增 `find_best_manager_for_sni` |
| `src/core/gateway/routes/tls/gateway_tls_routes.rs` | 新增 `from_routes_with_effective_hostnames` |
| `src/core/gateway/routes/tls/passthrough_proxy.rs` | 移除 `tls_route_manager` 字段，改为运行时 `find_best_manager_for_sni` |
| `examples/test/conf/TLSRoute/ListenerHostnameIntersection/` | 新建（YAML 配置） |
| `examples/code/client/suites/tls_route/listener_hostname_intersection/` | 新建（Rust suite） |
| `examples/code/client/suites/tls_route/mod.rs` | 添加 pub use |
| `examples/code/client/test_client.rs` | 注册 3 处 |
| `examples/test/conf/ports.json` | 新增端口 31291 |

---

## 边界情况与约束

- **HTTPRoute / GRPCRoute 不受影响**：`resolve_listener_keys()` 被 HTTP/gRPC route handler 共用，
  需确保改动对它们保持向后兼容。方案：新函数 `resolve_listeners()` 专用于 TLSRoute；
  原函数 `resolve_listener_keys()` 保留，供 HTTP/gRPC route handler 继续使用。
- **backward compat for serialization**：`ResolvedListener` 需实现 `Serialize/Deserialize`，
  且控制器推送给 gateway 的格式需与 gateway 反序列化兼容（二进制同步升级时可能出现旧格式）。
  短期内不需要版本兼容，因为 controller 和 gateway 同步发布。
- **`TlsRouteManager::rebuild()` 旧签名**：调用方（`rebuild_affected_listener_managers`）也需同步更新。
- **单元测试**：`route_utils.rs` 的现有单元测试需更新为使用新返回类型；
  `gateway_tls_routes.rs` 的现有测试需新增 effective hostname case。
