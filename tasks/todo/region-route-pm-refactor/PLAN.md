# RegionRoute PM 重构计划

## 完成说明（2026-04-14）

重构已全部实现并合并。实际实现与原始计划的关键差异：

| 差异点 | 原始计划 | 实际实现 |
|--------|---------|---------|
| Gateway 解析方式 | 预复制配置到 EP（Controller 侧 resolve） | Gateway 请求时实时查询 PluginMetaDataGatewayStore |
| PluginMetaDataGatewayStore | 未专门设计 | ArcSwap + pre-indexed typed maps（GlobalRegionRouteMap / ServiceRegionRouteMap），零克隆查找 |
| auto_failover | 保留（含 DNS probe、RegionHealthTable、probe_loop） | **完全删除**，仅剩静态 `failoverTo` |
| ServiceRegionRoute PM 创建 | 可能含 auto-create 路径 | **必须由管理员预建**，Controller 不自动创建 |
| GlobalRegionRoute 结构 | 含 `baseInfo` 包装层 | **无 `baseInfo`**，`myRegion`/`regions` 等字段直接位于 `config` 下 |
| 类型文件 | `plugin_metadata.rs`（单文件） | 拆分为 `plugin_metadata/mod.rs` + `plugin_metadata/region_route.rs` |
| Requeue for correctness | Controller requeue 影响 Gateway 正确性 | requeue 只影响 status conditions，Gateway 实时解析不依赖 requeue |
| Controller API 端点 | `global-region-confs`、`region-routes`、`region-groups` | 替换为 `global-region-pms`、`service-region-pms` 及对应 failover 端点 |
| Center API 端点 | `center/region-routes`、`center/region-groups/failover` | 替换为 `center/global-region-pms`、`center/service-region-pms` 及 failover 端点 |

文档更新（2026-04-14）：
- `skills/01-architecture/05-resources/15-plugin-metadata.md` — 更新源码路径、解析链说明、Gateway Store 结构、API 端点
- `docs/zh-CN/user-guide/edgion-plugins/region-route.md` — 完整重写，移除 baseInfo/autoFailover，新增三层架构说明
- `docs/en/user-guide/edgion-plugins/region-route.md` — 完整重写（同上）
- `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/region-route.md` — 更新示例
- `docs/en/user-guide/http-route/filters/edgion-plugins/region-route.md` — 更新示例

---

## 知识库入口

执行本计划前，按需查阅对应的 skills 系统（渐进加载，不要一次全读）：

| 项目 | Skills 入口 | 常用路径 |
|------|------------|---------|
| **Edgion 后端** | `Edgion/skills/SKILL.md` | 架构 → `01-architecture/`；资源系统 → `01-architecture/05-resources/`；插件开发 → `01-architecture/02-gateway/12-edgion-plugin-dev.md` |
| **edgion-dashboard 前端** | `edgion-dashboard/skills/SKILL.md` | 页面列表 → `skills/01-pages-and-routes.md`；API 对接 → `skills/02-api-conventions.md` |
| **工作区公共** | `/Users/caohao/ws2/skills/SKILL.md` | Center 测试 → `skills/05-center-testing/SKILL.md` |

重构以后端为主（Rust），前端 TODO 部分涉及 edgion-dashboard。执行时按实际任务范围查对应 skills，不需要全部预读。

---

## Git 分支策略

当前活跃分支 `feature-04-06` 含有其他不相关的提交，重构在独立分支上进行，避免历史混杂。

### 创建重构分支

```bash
cd /Users/caohao/ws2/Edgion
git checkout feature-04-06          # 确认起点
git checkout -b feature-region-route-pm-refactor
git diff feature-04-06              # 应无输出，确认起点一致
```

`feature-04-06` 的全部提交作为新分支的历史基础，不会出现在重构 diff 中：
- `git diff feature-04-06..HEAD` — 只看重构改动
- `git log feature-04-06..HEAD` — 只看重构 commits

### 完成后合并回主分支

```bash
git checkout feature-04-06
git merge feature-region-route-pm-refactor
```

### Commit 规范

每个 Step 独立 commit，便于 review 和回退：

```
feat: [Step1] refactor RegionRoute types — breaks compilation intentionally
feat: [Step2] update GlobalRegionRouteEntry/ServiceRegionRouteEntry
feat: [Step3a] update plugin_metadata handler
feat: [Step3b] rewrite resolve_region_route_plugins, remove auto-create
feat: [Step4] replace Controller API: global/service-region-pm endpoints
feat: [Step5] replace Center API: new region_pm_handlers
feat: [Step6] update Center DB schema and sync task for new PM endpoints
feat: [Step7] update CRD schema and test config YAMLs
feat: [Step8] update integration tests
chore: remove deprecated handlers and dead code
```

---

## 目标

将 RegionRoute 的管理层从"以 EdgionPlugins 为中心"彻底切换到"以 PluginMetaData 为中心"：

- **Global PM**（`GlobalRegionRoute`）：用 `namespace/name` 作为集群标识，去掉 `serviceGroup / cluster / clusterType` 冗余字段，扁平化结构，持有全量路由配置
- **Service PM**（`ServiceRegionRoute`）：由管理员手动创建，包含 `globalRef` 引用 Global PM，并持有 per-region failover override
- **EP**（`EdgionPlugins`）：只引用 Service PM（`serviceRegionRouteRef`），不再持有任何路由配置，也不引用 Global PM
- **API 层**：删除所有 EdgionPlugins 相关的 region-route 聚合接口，新增两组干净的 PM 接口
- **引用链**：`EP.serviceRegionRouteRef → Service PM → Service PM.globalRef → Global PM`

管理模型：管理员预先创建好 Global PM 和 Service PM，EP 用户直接引用 Service PM 名称即可。

---

## 一、类型定义变更

### 1.1 `src/types/resources/plugin_metadata.rs`

#### 变更：`GlobalRegionRouteConfig`

**现在（需要删除）：**
```rust
pub struct GlobalRegionRouteConfig {
    pub base_info: RegionRouteBaseInfo,   // ← 含 serviceGroup/cluster/clusterType
    pub key_get: Vec<KeyGet>,
    pub hash_key_get: Option<Vec<KeyGet>>,
    pub hash_calc: Option<HashCalcConfig>,
    pub route_rules: Vec<RouteRule>,
    pub auto_failover: Option<AutoFailoverConfig>,
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,
}
```

**重构后：**
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalRegionRouteConfig {
    /// Region definitions. PM namespace = cluster-type, PM name = cluster.
    #[serde(default)]
    pub regions: Vec<RegionDef>,

    /// Name of this gateway instance's own region.
    /// When routed to own region, skips external jump (pass-through).
    /// Empty = feature disabled.
    #[serde(default)]
    pub my_region: String,

    /// Shared key extraction for RouteByKeyConfMatch and RouteByHashRange.
    #[serde(default)]
    pub key_get: Vec<KeyGet>,

    /// Pre-computed hash key sources (tried before computing hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_key_get: Option<Vec<KeyGet>>,

    /// Hash computation config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_calc: Option<HashCalcConfig>,

    /// Ordered route rules — first match wins.
    #[serde(default)]
    pub route_rules: Vec<RouteRule>,

    /// Optional auto failover configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_failover: Option<AutoFailoverConfig>,

    /// RouteByKeyConfMatch configuration managed by Center.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,
}
```

#### 变更：`ServiceRegionRouteConfig`

**现在：**
```rust
pub struct ServiceRegionRouteConfig {
    pub regions: Vec<ServiceRegionDef>,
}
```

**重构后：**
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegionRouteConfig {
    /// Reference to the GlobalRegionRoute PM.
    /// Explicit linkage: namespace/name of the global PM this service PM belongs to.
    /// Set by the administrator when creating the Service PM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_ref: Option<PluginMetaDataRef>,

    /// Per-region failover state. Mirrors global PM's region names.
    /// Only entries with `failover_to` set have an active override.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<ServiceRegionDef>,
}
```

#### import 变更

删除对 `RegionRouteBaseInfo` 的所有引用（结构体已删除）：
```rust
// 删除所有:
//   use ... RegionRouteBaseInfo;
//   RegionRouteBaseInfo { ... }
//   config.base_info.xxx  →  改为 config.xxx
// RegionDef 仍然需要 import
use crate::types::resources::edgion_plugins::plugin_configs::region_route::RegionDef;
```

---

### 1.2 `src/types/resources/edgion_plugins/plugin_configs/region_route.rs`

#### 变更：`RegionRouteBaseInfo` → **完全删除**，`RegionRouteConfig` 精简为纯 Service PM ref

EP 不再承载任何路由配置，也不直接引用 Global PM，仅通过 `service_region_route_ref` 引用 Service PM。`RegionRouteBaseInfo` 结构体整体删除。

**现在：**
```rust
pub struct RegionRouteConfig {
    pub base_info: RegionRouteBaseInfo,              // ← 整体删除
    pub key_get: Vec<KeyGet>,                        // ← 删除
    pub hash_key_get: Option<Vec<KeyGet>>,           // ← 删除
    pub hash_calc: Option<HashCalcConfig>,           // ← 删除
    pub route_rules: Vec<RouteRule>,                 // ← 删除
    pub auto_failover: Option<AutoFailoverConfig>,   // ← 删除
    pub global_region_route_ref: Option<PluginMetaDataRef>, // ← 删除
    pub service_region_route_ref: Option<String>,
    pub validation_error: Option<String>,
}

pub struct RegionRouteBaseInfo { ... }               // ← 整体删除
```

**重构后：**
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteConfig {
    /// Reference to a ServiceRegionRoute PM by name.
    /// Namespace is inherited from the parent EdgionPlugins.
    /// The Service PM must be pre-created by the administrator;
    /// the Service PM in turn references the Global PM via its globalRef.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_region_route_ref: Option<String>,

    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}
// RegionRouteBaseInfo struct: 完全删除
```

EP 本身不持有任何路由配置。运行时 resolve 链：
```
EP.serviceRegionRouteRef → Service PM → Service PM.globalRef → Global PM
```

---

## 二、In-Memory Table 变更

### 2.1 `src/core/controller/conf_mgr/sync_runtime/resource_processor/region_route_table/global_table.rs`

#### 变更：`GlobalRegionRouteEntry`

**现在：**
```rust
pub struct GlobalRegionRouteEntry {
    pub pm_namespace: String,
    pub pm_name: String,
    pub service_group: String,         // ← 删除
    pub cluster: String,               // ← 删除
    pub cluster_type: Option<String>,  // ← 删除
    pub regions: Vec<RegionDef>,
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,
}
```

**重构后：**
```rust
pub struct GlobalRegionRouteEntry {
    pub pm_namespace: String,   // serves as "cluster type" dimension
    pub pm_name: String,        // serves as "cluster" identifier
    pub my_region: String,
    pub regions: Vec<RegionDef>,
    pub key_get: Vec<KeyGet>,
    pub hash_key_get: Option<Vec<KeyGet>>,
    pub hash_calc: Option<HashCalcConfig>,
    pub route_rules: Vec<RouteRule>,
    pub auto_failover: Option<AutoFailoverConfig>,
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,
}
```

`upsert_from_pm()` 直接从 `GlobalRegionRouteConfig` 新字段读取，不再读 `base_info`。

---

### 2.2 `src/core/controller/conf_mgr/sync_runtime/resource_processor/region_route_table/service_table.rs`

#### 变更：`ServiceRegionRouteEntry`

**现在：**
```rust
pub struct ServiceRegionRouteEntry {
    pub pm_namespace: String,
    pub pm_name: String,
    pub global_pm_ref: Option<PluginMetaDataRef>,
    pub regions: Vec<ServiceRegionDef>,
    pub ref_plugins: Vec<RefPluginEntry>,
}
```

**重构后**（结构体不变，`upsert_from_pm()` 改为从 `ServiceRegionRouteConfig.global_ref` 直接读取）：

```rust
pub struct ServiceRegionRouteEntry {
    pub pm_namespace: String,
    pub pm_name: String,
    /// Direct from ServiceRegionRouteConfig.global_ref — set when PM is loaded, not from EP
    pub global_pm_ref: Option<PluginMetaDataRef>,
    pub regions: Vec<ServiceRegionDef>,
    /// EdgionPlugins that reference this service PM (populated by plugin handler)
    pub ref_plugins: Vec<RefPluginEntry>,
}
```

删除 `set_global_pm_ref()` 方法（`global_pm_ref` 在 PM 加载时即确定，无需事后设置）。

---

## 三、Resource Processor 变更

### 3.1 `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugin_metadata.rs`

#### `update_region_route_tables()`

**Global PM 处理**：
```rust
// 旧：从 config.base_info 读 service_group/cluster/cluster_type/my_region/regions
// 新：直接从 config 读所有字段
fn upsert_global_entry(runtime, ns, name, config: &GlobalRegionRouteConfig) {
    runtime.global_region_route_table.upsert(GlobalRegionRouteEntry {
        pm_namespace: ns.to_string(),
        pm_name: name.to_string(),
        my_region: config.my_region.clone(),
        regions: config.regions.clone(),
        key_get: config.key_get.clone(),
        hash_key_get: config.hash_key_get.clone(),
        hash_calc: config.hash_calc.clone(),
        route_rules: config.route_rules.clone(),
        auto_failover: config.auto_failover.clone(),
        route_by_key_conf_match: config.route_by_key_conf_match.clone(),
    });
}
```

**Service PM 处理**：
```rust
// global_pm_ref 直接从 config.global_ref 读取
fn upsert_service_entry(runtime, ns, name, config: &ServiceRegionRouteConfig) {
    runtime.service_region_route_table.upsert(ServiceRegionRouteEntry {
        pm_namespace: ns.to_string(),
        pm_name: name.to_string(),
        global_pm_ref: config.global_ref.clone(),
        regions: config.regions.clone(),
        ref_plugins: vec![],  // populated later by EdgionPlugins handler
    });
}
```

---

### 3.2 `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`

#### `parse()` / `resolve_region_route_plugins()`

**变更点**：

- 删除 `set_global_pm_ref()` 调用（Service PM 的 `global_pm_ref` 在 PM 加载时已确定）
- 删除 `build_initial_plugin_metadata_yaml()` / Service PM 自动创建逻辑（Service PM 由管理员手动创建）
- `add_ref_plugin()` 调用保留（service table 仍需追踪哪些 EP 引用了这个 Service PM）

**新 resolve 流程**：

```rust
fn resolve_region_route_plugins(config: &mut RegionRouteConfig, ep_ns: &str, runtime: &Runtime) {
    let Some(ref svc_name) = config.service_region_route_ref else {
        // 没有引用 Service PM → 插件无配置，不生效
        config.validation_error = Some("serviceRegionRouteRef is required".to_string());
        return;
    };

    // 1. 查找 Service PM
    let svc_key = format!("{}/{}", ep_ns, svc_name);
    let Some(svc_entry) = runtime.service_region_route_table.get(&svc_key) else {
        config.validation_error = Some(format!(
            "service PM '{}/{}' not found; must be pre-created by administrator",
            ep_ns, svc_name,
        ));
        return;
    };

    // 2. 记录 EP → Service PM 引用（用于 UI 展示关联关系）
    runtime.service_region_route_table.add_ref_plugin(&svc_key, ep_ref);

    // 3. 查找 Global PM（通过 Service PM 的 global_ref）
    let Some(ref global_ref) = svc_entry.global_pm_ref else {
        config.validation_error = Some(format!(
            "service PM '{}/{}' has no globalRef configured",
            ep_ns, svc_name,
        ));
        return;
    };

    let global_ns = global_ref.namespace.as_deref().unwrap_or(ep_ns);
    let global_key = format!("{}/{}", global_ns, global_ref.name);
    let Some(global_entry) = runtime.global_region_route_table.get(&global_key) else {
        config.validation_error = Some(format!(
            "global PM '{}/{}' not found (referenced by service PM '{}/{}')",
            global_ns, global_ref.name, ep_ns, svc_name,
        ));
        return;
    };

    // 4. 从 Global PM 展开基础配置，再叠加 Service PM 的 failover override
    // （global_entry 和 svc_entry 传给 Gateway 的最终 resolved config）
    // 具体 resolved 结构见第九节
}
```

**删除的函数**：
- `build_initial_plugin_metadata_yaml()` — 完全删除
- `get_service_region_route_ref()` — 如果只用于 auto-create，也一并删除

---

## 四、Controller API 变更

### 4.1 删除的接口

在 `src/core/controller/api/region_route_handlers.rs` 中完全删除：
- `list_region_routes()` — 基于 EdgionPlugins 的分组列表
- `get_region_route_detail()` — EdgionPlugins 详情
- `list_region_groups()` — clusterType/cluster 聚合
- `list_global_region_confs()` — 旧版 global PM 接口（仅返回 name/namespace/baseInfo 子集）

在 `src/core/controller/api/mod.rs` 中删除对应路由注册。

---

### 4.2 新增的接口

在 `src/core/controller/api/region_route_handlers.rs` 中新增（从 in-memory table 直接读）：

#### `GET /api/v1/global-region-pms`

Response DTO：
```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalRegionPmEntry {
    pub namespace: String,
    pub name: String,
    pub my_region: String,
    pub regions: Vec<RegionDef>,
    pub key_get: serde_json::Value,
    pub hash_key_get: Option<serde_json::Value>,
    pub hash_calc: Option<serde_json::Value>,
    pub route_rules: serde_json::Value,
    pub route_by_key_conf_match: Option<serde_json::Value>,
}
```

实现逻辑：
```rust
pub async fn list_global_region_pms(State(state): State<Arc<AdminState>>) -> ... {
    let entries = state.runtime().global_region_route_table.all();
    let result: Vec<GlobalRegionPmEntry> = entries.into_iter().map(|e| GlobalRegionPmEntry {
        namespace: e.pm_namespace,
        name: e.pm_name,
        my_region: e.my_region,
        regions: e.regions,
        key_get: serde_json::to_value(&e.key_get).unwrap_or_default(),
        // ...
    }).collect();
    Ok(Json(ListResponse::success(result)))
}
```

#### `GET /api/v1/service-region-pms`

Response DTO：
```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegionPmEntry {
    pub namespace: String,
    pub name: String,
    pub global_ref: Option<PluginMetaDataRef>,
    pub regions: Vec<ServiceRegionDef>,
}
```

实现逻辑：类似，从 `service_region_route_table.all()` 读取。

#### `POST /api/v1/global-region-pms/failover`

供 Controller Ops 单集群界面使用，直接操作本 Controller 存储，无需 proxy。

Request body（与 Center failover 接口一致）：
```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPmFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    pub failover_to: String,  // 空字符串 = 清除
}
```

实现逻辑：GET 本地 PluginMetaData → 修改 `spec.plugins[GlobalRegionRoute].config.regions[region_name].failoverTo` → PUT 写回本地存储。

#### `POST /api/v1/service-region-pms/failover`

同上，操作 Service PM：修改 `spec.plugins[ServiceRegionRoute].config.regions[region_name].failoverTo`。

---

## 五、Center API 变更

### 5.1 删除的接口和文件

- 删除 `src/core/center/api/region_route_handlers.rs`（或清空重写）
- 删除 `src/core/center/api/region_group_handlers.rs`（或清空重写）

在 `src/core/center/api/mod.rs` 中删除对应路由注册：
```
DELETE /api/v1/center/region-routes
DELETE /api/v1/center/region-routes/{service_group}/{cluster}/{namespace}
DELETE /api/v1/center/region-groups
DELETE /api/v1/center/region-groups/failover
```

---

### 5.2 新增的接口

新建 `src/core/center/api/region_pm_handlers.rs`，包含 4 个接口：

#### `GET /api/v1/center/global-region-pms`

Fan-out 所有在线 Controller 的 `/api/v1/global-region-pms`，按 `(namespace, name)` 聚合：

Response DTO：
```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterGlobalRegionPm {
    pub namespace: String,
    pub name: String,
    pub my_region: String,
    pub regions: Vec<CenterRegionDef>,
    pub key_get: serde_json::Value,
    pub hash_key_get: Option<serde_json::Value>,
    pub hash_calc: Option<serde_json::Value>,
    pub route_rules: serde_json::Value,
    pub route_by_key_conf_match: Option<serde_json::Value>,
    pub controllers: Vec<String>,    // controller_ids that have this PM
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterRegionDef {
    pub name: String,
    pub hash_range: [u16; 2],
    pub failover_to: Option<String>,
}
```

聚合逻辑：
```
key = (namespace, name)
map[key].controllers.push(controller_id)
map[key].regions = merge_regions(existing, incoming)  // 以 region name 为 key 合并
```

---

#### `GET /api/v1/center/service-region-pms`

Fan-out 所有在线 Controller 的 `/api/v1/service-region-pms`，按 `(namespace, name)` 聚合：

Response DTO：
```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterServiceRegionPm {
    pub namespace: String,
    pub name: String,
    pub global_ref: Option<PluginMetaDataRef>,
    pub regions: Vec<ServiceRegionDef>,
    pub controllers: Vec<String>,
}
```

---

#### `POST /api/v1/center/global-region-pms/failover`

Request body：
```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPmFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    pub failover_to: String,  // 空字符串 = 清除
}
```

实现逻辑：
1. 向所有在线 Controller 请求 `/api/v1/global-region-pms`，找出含有 `(namespace, name)` 的 Controller
2. 对每个 Controller，通过 proxy GET `/api/v1/namespaced/PluginMetaData/{ns}/{name}`
3. 修改 `spec.plugins[GlobalRegionRoute].config.regions[region_name].failoverTo`
4. 通过 proxy PUT 写回

Response：
```json
{ "modified": 2, "failed": 0 }
```

---

#### `POST /api/v1/center/service-region-pms/failover`

Request body：
```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicePmFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    pub failover_to: String,  // 空字符串 = 清除
}
```

实现逻辑与 global failover 类似：
1. 找出有该 service PM 的 Controller
2. GET → 修改 `spec.plugins[ServiceRegionRoute].config.regions[region_name].failoverTo`
3. PUT 写回

---

## 六、路由注册变更

### `src/core/center/api/mod.rs`

删除：
```rust
.route("/api/v1/center/region-routes", get(region_route_handlers::list_center_region_routes))
.route("/api/v1/center/region-routes/{service_group}/{cluster}/{namespace}", get(...))
.route("/api/v1/center/region-groups", get(region_group_handlers::list_center_region_groups))
.route("/api/v1/center/region-groups/failover", post(region_group_handlers::region_failover))
```

新增：
```rust
.route("/api/v1/center/global-region-pms", get(region_pm_handlers::list_center_global_region_pms))
.route("/api/v1/center/global-region-pms/failover", post(region_pm_handlers::global_pm_failover))
.route("/api/v1/center/service-region-pms", get(region_pm_handlers::list_center_service_region_pms))
.route("/api/v1/center/service-region-pms/failover", post(region_pm_handlers::service_pm_failover))
```

### `src/core/controller/api/mod.rs`

删除：
```rust
.route("/api/v1/region-routes", get(region_route_handlers::list_region_routes))
.route("/api/v1/region-routes/{service_group}/{cluster}/{namespace}/{kind}", get(...))
.route("/api/v1/region-groups", get(region_route_handlers::list_region_groups))
.route("/api/v1/global-region-confs", get(region_route_handlers::list_global_region_confs))
```

新增：
```rust
.route("/api/v1/global-region-pms", get(region_route_handlers::list_global_region_pms))
.route("/api/v1/service-region-pms", get(region_route_handlers::list_service_region_pms))
```

---

## 七、CRD Schema 变更（`config/crd/edgion-crd/plugin_metadata_crd.yaml`）

GlobalRegionRoute plugins.config 字段：

```yaml
# 删除 baseInfo 包装层，直接放到 config 下：
config:
  type: object
  properties:
    regions:              # 原 baseInfo.regions，直接提升
      type: array
      ...
    myRegion:             # 原 baseInfo.myRegion，直接提升
      type: string
    keyGet:               # 不变
      ...
    hashKeyGet:           # 不变
      ...
    hashCalc:             # 不变
      ...
    routeRules:           # 不变
      ...
    routeByKeyConfMatch:  # 不变
      ...
# 删除 baseInfo, serviceGroup, cluster, clusterType
```

ServiceRegionRoute plugins.config 字段：

```yaml
config:
  type: object
  properties:
    globalRef:            # 新增
      type: object
      properties:
        namespace:
          type: string
        name:
          type: string
    regions:              # 不变
      type: array
      items:
        type: object
        required: [name]
        properties:
          name:
            type: Cargo.lockstring
          failoverTo:
            type: string
```

EdgionPlugins RegionRoute plugins.config 字段：

```yaml
# 大幅精简，只保留：
config:
  type: object
  properties:
    serviceRegionRouteRef:  # 仅保留此字段
      type: string
# 删除：baseInfo, keyGet, hashKeyGet, hashCalc, routeRules, autoFailover, globalRegionRouteRef
```

---

## 八、测试配置文件变更

### `examples/test/conf/EdgionPlugins/RegionRoute/01_PluginMetaData_global-region-conf.yaml`

旧格式：
```yaml
spec:
  plugins:
    - type: GlobalRegionRoute
      config:
        baseInfo:
          serviceGroup: region-route-test
          cluster: test
          regions: [...]
        keyGet: [...]
        routeRules: [...]
```

新格式：
```yaml
spec:
  plugins:
    - type: GlobalRegionRoute
      config:
        regions: [...]       # 直接放 config 下
        myRegion: ""
        keyGet: [...]
        routeRules: [...]
```

### `examples/test/conf/EdgionPlugins/RegionRoute/01_PluginMetaData_region-route-main.yaml`

旧格式：
```yaml
spec:
  plugins:
    - type: ServiceRegionRoute
      config:
        regions:
          - name: virginia
          - name: ohio
```

新格式：
```yaml
spec:
  plugins:
    - type: ServiceRegionRoute
      config:
        globalRef:                    # 新增，管理员配置
          namespace: edgion-default
          name: global-region-conf
        regions:
          - name: virginia
          - name: ohio
```

### `examples/test/conf/EdgionPlugins/RegionRoute/` 下的 EdgionPlugins 文件

旧格式（含内联路由配置）：
```yaml
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        baseInfo:
          serviceGroup: region-route-test
          cluster: test
          regions: [...]
        keyGet: [...]
        routeRules: [...]
        globalRegionRouteRef:
          name: global-region-conf
        serviceRegionRouteRef: region-route-main
```

新格式（仅引用 Service PM）：
```yaml
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        serviceRegionRouteRef: region-route-main
```

`ctrl2-inconsistent` 目录下的相关文件也需同样更新。

---

## 九、Gateway Failover 链（`src/core/gateway/plugins/http/region_route/plugin.rs`）

Gateway 运行时逻辑不变。Gateway 收到的最终 resolved config 由 Controller 在 resolve 阶段生成。

**Controller resolve 阶段产出的最终配置**（传给 Gateway，格式不变）：

resolve 后，Gateway 拿到的是已展开的完整路由配置（从 Global PM 展开，Service PM failover 叠加），其中包含：
- `regions`（含最终 failoverTo，Service PM override 已合并进每个 region）
- `my_region`
- `key_get / hash_calc / route_rules`
- `auto_failover`（直接从 Global PM 原样复制，Gateway 用于健康探测驱动的动态 failover）

**三种 failover 机制说明**：

| 机制 | 来源 | 设置方式 | 生效时机 |
|------|------|---------|---------|
| `Service PM region.failoverTo` | Service PM | 管理员/Center failover API 写入 | 静态，每次请求都走 |
| `Global PM region.failoverTo` | Global PM | Center failover API 写入 | 静态，Service PM 未设置时生效 |
| `auto_failover`（DNS 健康探测） | Global PM config | 管理员在 GlobalRegionRoute 配置 | 动态，Gateway 探测到 region 不健康时触发 |

优先级：`Service PM.failoverTo > Global PM.failoverTo > auto_failover`

`auto_failover` 字段仍然用户可配置（在 GlobalRegionRoute PM 的 config 中），此次重构不改变其语义，Controller resolve 时原样传给 Gateway。

**Controller resolve 流程更新**（`edgion_plugins.rs` `resolve_region_route_plugins()`）：

```
旧：EP.globalRegionRouteRef → Global PM → 叠加 EP.serviceRegionRouteRef → Service PM failover
新：EP.serviceRegionRouteRef → Service PM → Service PM.globalRef → Global PM → 叠加 Service PM failover
```

---

## 十、集成测试变更

### `examples/code/client/suites/edgion_plugins/region_route/region_route.rs`

- 更新 PM 字段断言：不再查 `base_info.serviceGroup`，而是查 `regions` 在顶层
- 删除对 `/api/v1/region-routes` 端点的调用
- 新增对 `/api/v1/global-region-pms` / `/api/v1/service-region-pms` 的断言
- EP 配置只验证 `serviceRegionRouteRef` 字段

### `examples/code/client/suites/edgion_plugins/plugin_metadata_managed/plugin_metadata_managed.rs`

- 删除 `auto_create_plugin_metadata` 相关测试（Service PM 不再自动创建）
- 新增：验证 Service PM 缺失时 EP 的 `validationError` 正确设置

---

## 十一、执行策略与顺序

### 策略：整体重写，按编译依赖顺序推进

本次重构变化范围大（EP 字段从 8 个缩减到 1 个、resolve 链逻辑完全翻转、auto-create 逻辑整体删除、API 全部换新端点），建议**整体重写**而非增量修改。

重写优势：
- 每步 `cargo check` 后，编译错误精确指向还未改的使用点，不会漏改
- 避免"旧代码+新代码"混杂导致的逻辑矛盾
- 删除死代码更彻底，clippy 警告更少

---

### Step 1 — 类型层（无上游依赖）

**文件**：`src/types/resources/plugin_metadata.rs`
- `GlobalRegionRouteConfig`：删除 `base_info: RegionRouteBaseInfo`，将 `regions`/`my_region` 直接放到结构体顶层，其余字段不变
- `ServiceRegionRouteConfig`：新增 `global_ref: Option<PluginMetaDataRef>`
- import：删除对 `RegionRouteBaseInfo` 的引用，改为直接 import `RegionDef`

**文件**：`src/types/resources/edgion_plugins/plugin_configs/region_route.rs`
- `RegionRouteConfig`：删除所有路由字段和 `global_region_route_ref`，只保留 `service_region_route_ref` 和 `validation_error`
- 删除 `RegionRouteBaseInfo` 结构体整体
- 删除 `validate()` 中所有 `base_info` 相关校验（新 EP 结构无需本地校验，PM 不存在时 resolve 阶段报错）

执行：`cargo check` → 此时会爆出所有下游使用点，逐一修复。

---

### Step 2 — In-Memory Table 层（依赖 Step 1 类型）

**文件**：`src/core/controller/conf_mgr/sync_runtime/resource_processor/region_route_table/global_table.rs`
- `GlobalRegionRouteEntry`：删除 `service_group`/`cluster`/`cluster_type`，新增 `my_region`/`key_get`/`hash_key_get`/`hash_calc`/`route_rules`/`auto_failover`
- `upsert_from_pm()`：改为从 `GlobalRegionRouteConfig` 新字段直接读取

**文件**：`src/core/controller/conf_mgr/sync_runtime/resource_processor/region_route_table/service_table.rs`
- `ServiceRegionRouteEntry`：结构体不变，但 `upsert_from_pm()` 改为从 `ServiceRegionRouteConfig.global_ref` 读取 `global_pm_ref`
- 删除 `set_global_pm_ref()` 方法

执行：`cargo check`。

---

### Step 3 — Resource Processor（依赖 Step 1+2）

**文件**：`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugin_metadata.rs`
- `update_region_route_tables()` 中 Global PM 处理：按 `GlobalRegionRouteConfig` 新字段重写 `upsert_global_entry()`
- Service PM 处理：按 `ServiceRegionRouteConfig.global_ref` 重写 `upsert_service_entry()`

**文件**：`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
- **全部重写** `resolve_region_route_plugins()`，新 resolve 链：
  ```
  EP.service_region_route_ref
    → Service PM（不存在则 validation_error）
    → Service PM.global_pm_ref
      → Global PM（不存在则 validation_error）
    → 展开 Global PM 配置 + 叠加 Service PM failover override
  ```
- 删除 `build_initial_plugin_metadata_yaml()`（Service PM 不再自动创建）
- 删除 `get_service_region_route_ref()` 辅助函数（如仅用于 auto-create）
- 删除 `set_global_pm_ref()` 调用
- `add_ref_plugin()` 保留（追踪 EP → Service PM 引用关系）

执行：`cargo check`。

---

### Step 4 — Controller API（依赖 Step 2 table）

**文件**：`src/core/controller/api/region_route_handlers.rs`
- 删除：`list_region_routes()`、`get_region_route_detail()`、`list_region_groups()`、`list_global_region_confs()`
- 新增：`list_global_region_pms()`（从 `global_region_route_table.all()` 读）
- 新增：`list_service_region_pms()`（从 `service_region_route_table.all()` 读）

**文件**：`src/core/controller/api/mod.rs`
- 删除旧路由，注册新路由

执行：`cargo check`。

---

### Step 5 — Center API（依赖 Step 4）

**文件**：`src/core/center/api/region_route_handlers.rs` — 清空重写（或删除）
**文件**：`src/core/center/api/region_group_handlers.rs` — 清空重写（或删除）
**新建**：`src/core/center/api/region_pm_handlers.rs`，实现 4 个接口：
- `list_center_global_region_pms()` — fan-out + 按 (namespace, name) 聚合
- `list_center_service_region_pms()` — fan-out + 按 (namespace, name) 聚合
- `global_pm_failover()` — proxy GET → 修改 → proxy PUT
- `service_pm_failover()` — proxy GET → 修改 → proxy PUT

**文件**：`src/core/center/api/mod.rs` — 更新路由注册

执行：`cargo check`。

---

### Step 6 — Center DB 层

**文件**：`src/core/center/db/mod.rs`

旧 `region_route_cache` 表以 `(service_group, cluster, namespace, kind, resource_name)` 为 key，所有这些字段在新架构中已不存在。需要替换为按 PM namespace/name 缓存：

```sql
-- 删除旧表
DROP TABLE IF EXISTS region_route_cache;

-- 新增 Global PM 缓存表
CREATE TABLE IF NOT EXISTS global_pm_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    controller_id TEXT NOT NULL,
    pm_namespace TEXT NOT NULL,
    pm_name TEXT NOT NULL,
    entries_json TEXT NOT NULL DEFAULT '{}',
    synced_at INTEGER NOT NULL DEFAULT 0,
    UNIQUE(controller_id, pm_namespace, pm_name)
);

-- 新增 Service PM 缓存表
CREATE TABLE IF NOT EXISTS service_pm_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    controller_id TEXT NOT NULL,
    pm_namespace TEXT NOT NULL,
    pm_name TEXT NOT NULL,
    entries_json TEXT NOT NULL DEFAULT '{}',
    synced_at INTEGER NOT NULL DEFAULT 0,
    UNIQUE(controller_id, pm_namespace, pm_name)
);
```

`CenterDb` 对应方法变更：
- 删除：`upsert_region_route_cache()` / `get_cached_region_route_detail()` / `clear_region_route_cache()`
- 新增：`upsert_global_pm_cache(ctrl_id, pm_ns, pm_name, entries_json)`
- 新增：`upsert_service_pm_cache(ctrl_id, pm_ns, pm_name, entries_json)`
- 新增：`list_global_pm_cache(ctrl_id)` → `Vec<DbPmCacheEntry>`（可选，供 Center 离线展示用）
- `controllers` 表保持不变

**文件**：`src/core/center/db/sync.rs`

后台同步任务改为拉新接口：

```rust
// 旧：GET /api/v1/region-routes  (列出所有 route keys)
// 旧：GET /api/v1/region-routes/{sg}/{cluster}/{ns}/{kind}
// 新：GET /api/v1/global-region-pms  (直接拿列表)
// 新：GET /api/v1/service-region-pms
```

同步逻辑简化：不再需要两步（list keys → fetch detail），直接一次性拿完整 PM 列表，按 `(pm_namespace, pm_name)` upsert 进 SQLite。

```rust
// DTO 匹配新接口响应
#[derive(serde::Deserialize)]
struct GlobalRegionPmEntry {
    namespace: String,
    name: String,
    // ... 其余字段用 serde_json::Value 接收即可
}

async fn sync_one_controller(proxy, db, controller_id) {
    // 1. 拉 Global PM 列表
    let global_pms = fetch_json(proxy, controller_id, "/api/v1/global-region-pms").await;
    for pm in global_pms {
        let json = serde_json::to_string(&pm).unwrap_or_default();
        db.upsert_global_pm_cache(controller_id, &pm.namespace, &pm.name, &json);
    }

    // 2. 拉 Service PM 列表
    let service_pms = fetch_json(proxy, controller_id, "/api/v1/service-region-pms").await;
    for pm in service_pms {
        let json = serde_json::to_string(&pm).unwrap_or_default();
        db.upsert_service_pm_cache(controller_id, &pm.namespace, &pm.name, &json);
    }
}
```

**SQLite migration 处理**：

项目目前无版本号 migration 机制，直接在 `run_migrations()` 的 `SCHEMA_SQL` 中处理：

```rust
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS controllers ( ... );  -- 保持不变

-- 删除旧表（兼容旧数据库文件）
DROP TABLE IF EXISTS region_route_cache;

-- 新建 Global PM 缓存表
CREATE TABLE IF NOT EXISTS global_pm_cache ( ... );

-- 新建 Service PM 缓存表
CREATE TABLE IF NOT EXISTS service_pm_cache ( ... );
"#;
```

`DROP TABLE IF EXISTS` 是幂等的，旧库和新库都能安全执行。旧缓存数据丢弃无影响，下次 sync 任务运行后会自动重新填充。

---

### Step 7 — YAML 文件

- `config/crd/edgion-crd/plugin_metadata_crd.yaml`：GlobalRegionRoute 去掉 baseInfo，ServiceRegionRoute 加 globalRef，EP RegionRoute 精简为只剩 serviceRegionRouteRef
- `examples/test/conf/EdgionPlugins/RegionRoute/` 下所有配置文件：
  - Global PM YAML：去掉 baseInfo 包装层
  - Service PM YAML：加 globalRef
  - EdgionPlugins YAML：删除所有内联路由配置，只保留 serviceRegionRouteRef

---

### Step 8 — 集成测试 + 全量检查（依赖 Step 1–7 全部完成）

**文件**：`examples/code/client/suites/edgion_plugins/region_route/region_route.rs`
- 删除对旧端点的调用
- 新增对 `/api/v1/global-region-pms` / `/api/v1/service-region-pms` 的断言
- EP 配置只验证 `serviceRegionRouteRef`

**文件**：`examples/code/client/suites/edgion_plugins/plugin_metadata_managed/plugin_metadata_managed.rs`
- 删除 auto-create 测试
- 新增：Service PM 缺失时 EP `validationError` 验证

全量检查：
```bash
cargo fmt --all
cargo check --all-targets
cargo clippy --all-targets
make check-agent-docs
```

---

## 十二、不改变的部分

- `RegionDef`（region_route.rs）：保持不变（name/hashRange/backendEndpoint/tls/failoverTo）
- `ServiceRegionDef`（plugin_metadata.rs）：保持不变（name/failoverTo）
- `PluginMetaDataRef`：保持不变
- `RouteByKeyConfMatchMeta`：保持不变
- Gateway 运行时插件逻辑：不变
- gRPC 配置同步格式：不变（Controller resolve 后的 final config 格式不变）

---

## 十三、受变更影响的访问点（需全部修改）

### EP `RegionRouteConfig` 字段变更

| 旧写法 | 处理方式 |
|--------|---------|
| `config.base_info.regions` | 删除，改从 Global PM entry 读 |
| `config.base_info.my_region` | 删除，改从 Global PM entry 读 |
| `config.base_info.service_group` | 删除，无对应字段 |
| `config.base_info.cluster` | 删除，无对应字段 |
| `config.base_info.cluster_type` | 删除，无对应字段 |
| `config.key_get` | 删除，改从 Global PM entry 读 |
| `config.hash_calc` | 删除，改从 Global PM entry 读 |
| `config.route_rules` | 删除，改从 Global PM entry 读 |
| `config.auto_failover` | 删除，改从 Global PM entry 读 |
| `config.global_region_route_ref` | 删除，Global PM 通过 Service PM 间接获取 |

### `GlobalRegionRouteConfig` 字段变更

| 旧写法 | 新写法 |
|--------|--------|
| `config.base_info.regions` | `config.regions` |
| `config.base_info.my_region` | `config.my_region` |
| `RegionRouteBaseInfo { regions, my_region, .. }` | 直接在 `GlobalRegionRouteConfig` 上构造 |

需要检查的文件：
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugin_metadata.rs`
- `src/core/controller/api/region_route_handlers.rs`
- `src/core/gateway/plugins/http/region_route/plugin.rs`
- 所有集成测试代码

---

## 十四、TODO — 前端 Center：GPM / SPM 创建与修改

### 背景

当前 Center 的 Region Route 页面仅做展示。需新增 GPM 和 SPM 的创建/修改能力，由管理员在 Center 统一操作并一键下发到各 Controller。

### GPM（Global PM）管理

**创建 GPM**：
- 表单字段：namespace（作为 cluster-type）、name（作为 cluster 标识）、regions 列表（name/hashRange/backendEndpoint/tls）、myRegion、keyGet、hashCalc、routeRules、autoFailover
- **关联集群**：创建时必须选择要下发的 Controller 集群（多选），不可为空
- **一键下发**：提交后，Center 向选中的每个 Controller proxy PUT `/api/v1/namespaced/PluginMetaData/{ns}/{name}`，所有 Controller 统一写入同一份 GPM

**修改 GPM**：
- 在 GPM 列表选中一条，编辑路由配置（regions/routeRules 等）
- 下发范围：默认对已有该 GPM 的所有 Controller 执行更新（可取消勾选部分）
- failover 操作（修改单个 region.failoverTo）保持现有接口 `POST /api/v1/center/global-region-pms/failover`

### SPM（Service PM）管理

**创建 SPM**：
- 表单字段：namespace、name、regions 列表（name + 可选 failoverTo）
- **必须关联 GPM**：选择已有的 GPM（namespace + name），写入 `globalRef`
- 关联后，regions 列表从所选 GPM 的 regions 预填（只允许设置 failoverTo，不允许新增/删除 region）
- **关联集群**：选择要下发的 Controller 集群（多选），与所选 GPM 存在的 Controller 保持一致（默认全选）
- **一键下发**：Center 向选中的每个 Controller proxy PUT `/api/v1/namespaced/PluginMetaData/{ns}/{name}`

**修改 SPM**：
- 编辑各 region 的 failoverTo（即 per-region failover override）
- 下发范围：默认对已有该 SPM 的所有 Controller 执行更新

### 后端接口补充

需要在 Center 新增以下写接口（在 `region_pm_handlers.rs` 中实现）：

```
POST   /api/v1/center/global-region-pms/create    创建 GPM 并下发到指定 Controller
PUT    /api/v1/center/global-region-pms/update    修改 GPM 并下发到指定 Controller
POST   /api/v1/center/service-region-pms/create   创建 SPM 并下发到指定 Controller
PUT    /api/v1/center/service-region-pms/update   修改 SPM 并下发到指定 Controller
```

Request body 均包含 `controllers: Vec<String>`（下发的 controller_id 列表），其余字段为 PM 内容。

---

## 十五、TODO — 前端 Controller Ops：单集群 RegionRoute 管理

### 背景

Controller 的 ops 界面（单集群视角）也需要支持 RegionRoute，用于管理员对当前集群进行查看和操作，不跨集群。

### 功能范围

**GPM 列表**（只读展示）：
- 展示当前 Controller 的 `/api/v1/global-region-pms` 数据
- 字段：namespace（cluster-type）、name（cluster）、regions、routeRules 概览

**GPM failover**：
- 对单个 region 设置/清除 `failoverTo`
- 调用 Controller 本地接口（待补充，目前 failover 只有 Center 侧）
- 需要在 Controller API 新增：`POST /api/v1/global-region-pms/failover`（与 Center 侧 body 一致，但只操作本 Controller）

**SPM 列表**（只读展示）：
- 展示当前 Controller 的 `/api/v1/service-region-pms` 数据
- 字段：namespace、name、globalRef、per-region failoverTo

**SPM failover**：
- 对单个 region 设置/清除 `failoverTo`
- 需要在 Controller API 新增：`POST /api/v1/service-region-pms/failover`

### 后端接口补充（Controller 侧）

```
POST /api/v1/global-region-pms/failover    修改当前集群 GPM 的 region.failoverTo
POST /api/v1/service-region-pms/failover   修改当前集群 SPM 的 region.failoverTo
```

实现逻辑：直接对本地存储执行 GET → 修改 → PUT，无需 proxy。
