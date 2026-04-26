---
name: resource-plugin-metadata
description: PluginMetaData 资源：插件体系（类 EdgionPlugins），支持 Global RegionConf + Service 级状态管理。
---

# PluginMetaData 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

PluginMetaData 是 Edgion 的自定义扩展资源，承载一个 `MetaDataEntry` 变体（tagged union，判别字段 `spec.metadata.type`）。当前共 **5 种**变体：

1. **数据列表**：`KeyList`（统一了旧的 StringList + RegexList，按 `matchMode: Exact|Regex` 区分）、`IpList`（CIDR 列表）
2. **RegionRoute 体系**：`ClusterRegionRoute`（完整拓扑 + 路由规则）、`ServiceRegionRoute`（服务级 failover，通过 `clusterRef` 引用 ClusterRegionRoute PM）
3. **L4 连接限制**：`GlobalConnectionIpRestriction`（被 EdgionStreamPlugins 通过 `pluginMetadataRef` 引用）

## 源码位置

- 类型定义: `src/types/resources/plugin_metadata/mod.rs`（主模块）、`src/types/resources/plugin_metadata/region_route.rs`（RegionRoute 配置结构体）
- Plugin 类型: `MetaDataPlugin` enum（ClusterRegionRoute / ServiceRegionRoute 变体）
- Controller Handler: `handlers/plugin_metadata.rs`
- EdgionPlugins Handler（验证逻辑）: `handlers/edgion_plugins.rs`
- PluginMetaDataStore（Controller 侧）: `plugin_metadata_utils/plugin_metadata_store.rs`
- PluginMetaDataGatewayStore（Gateway 侧）: `src/core/gateway/plugins/runtime/plugin_metadata/conf_handler.rs`
- RegionRoute 表（Controller）: `region_route_table/cluster_table.rs`、`region_route_table/service_table.rs`
- Controller API: `src/core/controller/api/region_route_handlers.rs`
- Center API（列表 + failover）: `src/core/center/api/region_route_handlers.rs`
- Center API（一致性）: `src/core/center/api/consistency_handlers.rs`
- Center 元数据存储: `src/core/center/metadata_store/` （CenterMetaDataStore，内存强类型，按 Controller 聚合）
- Center Watch 缓存: `src/core/center/watch_cache/`（反向 list/watch，接收 Controller gRPC 推送）
- Center ConfHandler: `src/core/center/api/metadata_conf_handler.rs`

## Plugin 体系

`spec.plugins` 是一个 tagged union 数组，格式：

```yaml
spec:
  plugins:
    - type: ClusterRegionRoute    # 集群拓扑 + 路由规则（无 baseInfo 包装）
      config:
        myRegion: ...
        regions: [...]
        keyGet: [...]
        routeRules: [...]
    - type: ServiceRegionRoute   # 服务级 failover + clusterRef 引用
      config:
        clusterRef:
          name: global-region-conf
          namespace: edgion-system
        regions: [...]
```

### MetaDataEntry 枚举（5 变体）

| 变体 | 用途 | 外部引用 |
|------|------|---------|
| `KeyList` | 键值列表，支持 `Exact` / `Regex` 匹配模式（被 KeyAuth、IpRestriction 等插件间接消费）| 无 |
| `IpList` | CIDR 列表（IP 黑白名单类插件）| 无 |
| `ClusterRegionRoute` | 集群拓扑 + 路由规则 + matchMap（运维/Center 管理） | 无 |
| `ServiceRegionRoute` | 服务级 failover + `clusterRef` 引用一个 ClusterRegionRoute PM | `clusterRef` → ClusterRegionRoute PM |
| `GlobalConnectionIpRestriction` | L4 连接阶段 IP 限制，带 profile 切换能力（Center-managed，被 EdgionStreamPlugins 消费）| 无 |

## RegionRoute 两层架构

```
ClusterRegionRoute PluginMetaData ←── ServiceRegionRoute PluginMetaData
│ myRegion                           │ clusterRef → ClusterRegionRoute PluginMetaData
│ regions（完整拓扑）                 │ regions（per-region failoverTo 覆盖）
│ keyGet                             │
│ hashCalc                           │
│ routeRules                         │
│ routeByKeyConfMatch（matchMap）      │
│ （无 baseInfo 包装，无 autoFailover）│
└─────────────────────────────────── EdgionPlugins.serviceRegionRouteRef ──┘
                                     （RegionRoute plugin，只有一个 ref）
```

### 解析链（Gateway 请求时实时查询）

```
EP.serviceRegionRouteRef
  → ServiceRegionRoute PluginMetaData（名称在 EP 命名空间下查找）
    → ServiceRegionRoute PluginMetaData.clusterRef
      → ClusterRegionRoute PluginMetaData（namespace/name）
```

Gateway 在请求时从全局 `PluginMetaDataGatewayStore`（ArcSwap + pre-indexed typed maps）中实时解析此链，无锁、零克隆读取。配置**不**由 Controller 预复制进 EP，而是 Gateway 按需查询。

### ClusterRegionRoute PluginMetaData（集群配置）

由 Center/运维预建，所有 RegionRoute 共享。**不含 `baseInfo` 包装**，所有字段直接位于 `config` 下，也不含 `autoFailover`（已移除）。

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: global-region-conf
  namespace: edgion-system
spec:
  plugins:
    - type: ClusterRegionRoute
      config:
        myRegion: "virginia"
        regions:
          - name: virginia
            hashRange: [0, 599]
            backendEndpoint: "virginia.example.com"
          - name: ohio
            hashRange: [600, 999]
            backendEndpoint: "ohio.example.com"
        keyGet:
          - type: header
            name: X-Account-Id
        hashCalc:
          algorithm: crc32
          modulo: 1000
        routeRules:
          - type: RouteByKeyConfMatch
          - type: RouteByHashRange
        routeByKeyConfMatch:
          matchMap:
            "global-key": virginia
```

### ServiceRegionRoute PluginMetaData（业务配置）

**必须由管理员预先创建**，Controller 不会自动生成。包含 `clusterRef` 指向对应的 ClusterRegionRoute PluginMetaData。

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: my-svc-pm
  namespace: default
spec:
  plugins:
    - type: ServiceRegionRoute
      config:
        clusterRef:
          name: global-region-conf
          namespace: edgion-system
        regions:
          - name: virginia
            failoverTo: ohio
```

### EdgionPlugins 引用

EdgionPlugins 中的 `RegionRouteConfig` 只有 `serviceRegionRouteRef` 一个字段，不含任何内联路由配置：

```yaml
- type: RegionRoute
  config:
    serviceRegionRouteRef: my-svc-pm
```

`serviceRegionRouteRef` 是 EP 所在命名空间下的 ServiceRegionRoute PluginMetaData 名称（纯字符串，无 namespace 字段）。

### Gateway 实时解析（PluginMetaDataGatewayStore）

Gateway 不缓存解析结果，每次请求通过 `PluginMetaDataGatewayStore` 实时走完引用链：

- **存储结构**：ArcSwap 持有 pre-indexed typed maps（`ClusterRegionRouteMap`、`ServiceRegionRouteMap`），key 为 `"namespace/name"`
- **查找**：`get_cluster_region_route(ns, name)` / `get_service_region_route(ns, name)` — ArcSwap load + HashMap get，零克隆
- **更新**：ConfHandler 收到增量时重建 typed maps 并 ArcSwap store，读路径无锁

### 合并优先级（Gateway 最终配置）

| 优先级 | 来源 | 说明 |
|--------|------|------|
| 1 | ClusterRegionRoute PluginMetaData 全部配置 | `regions`, `keyGet`, `hashCalc`, `routeRules`, `matchMap` |
| 2 | ServiceRegionRoute PluginMetaData `regions[].failoverTo` | 设置 `region.failover_to`（服务级覆盖） |

> `autoFailover` 已从系统中完全移除：DNS 健康探测、RegionHealthTable、probe_loop 等全部删除。仅剩静态 `failoverTo` 重定向。

### Center 操作

> Center 通过反向 list/watch（联邦 gRPC 流）从各 Controller 接收 PM 数据，存入 `CenterMetaDataStore`（内存强类型，按 Controller 聚合）。Center **不再**主动向 Controller 轮询 HTTP 接口获取 PM 数据。

| 操作 | 目标 | 端点 |
|------|------|------|
| 列出集群 RegionRoute | ClusterRegionRoute PM 聚合列表 | `GET /api/v1/center/cluster-region-routes` |
| 列出服务 RegionRoute | ServiceRegionRoute PM 聚合列表 | `GET /api/v1/center/service-region-routes` |
| 集群 region failover（静态） | ClusterRegionRoute PluginMetaData `regions[].failoverTo` | `POST /api/v1/center/cluster-region-routes/failover` |
| 服务级 failover | ServiceRegionRoute PluginMetaData `regions[].failoverTo` | `POST /api/v1/center/service-region-routes/failover` |
| 集群一致性检查 | 各 Controller 数据一致性比对 | `GET /api/v1/center/cluster-region-routes/consistency` |
| 服务一致性检查 | 各 Controller 数据一致性比对 | `GET /api/v1/center/service-region-routes/consistency` |
| 集群 matchMap | ClusterRegionRoute PluginMetaData `routeByKeyConfMatch.matchMap` | 通过 proxy PUT PluginMetaData |

### Controller API

| 端点 | 说明 |
|------|------|
| `GET /api/v1/cluster-region-routes` | 列出所有 ClusterRegionRoute PluginMetaData |
| `GET /api/v1/service-region-routes` | 列出所有 ServiceRegionRoute PluginMetaData |
| `POST /api/v1/cluster-region-routes/failover` | 修改 Cluster PM 中某 region 的 failoverTo |
| `POST /api/v1/service-region-routes/failover` | 修改 Service PM 中某 region 的 failoverTo |

> **接口契约**：上述 failover 接口（Center 与 Controller 两端）的 body 必须严格是
> `{namespace, name, regionName, failoverTo}` 四个字段，含任何其他字段（例如 `myRegion`、
> `spec`、完整 PluginMetaData YAML）都会在反序列化阶段被 4xx 拒绝。这条契约由强类型
> `FailoverRequest` + `deny_unknown_fields` 在两端独立强制：Center 重新序列化后再 fan-out
> 给 Controller，绝不透传上游 raw bytes。

## Controller 侧处理

### preparse

- 调 `PluginMetaData::validate_pre_parse()` 收集 per-variant 校验错误（invalid CIDR、invalid regex、GCIR 的 `validate_and_init()` 失败等），返回 `Vec<String>`，作为 `Accepted=False/Invalid` 的依据

### parse

- 更新 `PluginMetaDataStore`（供 EdgionPlugins、EdgionStreamPlugins handler 解析跨 PM 引用时查找）
- 更新 `RegionRouteClusterTable`（`ClusterRegionRoute` 变体）或 `RegionRouteServiceTable`（`ServiceRegionRoute` 变体）
- **Cross-PM ref 注册**：对 `ServiceRegionRoute` 变体，将 `cluster_ref` 注册到 `plugin_metadata_refs`，key 格式 `"<cluster_ns>/<cluster_name>"`，value 为 `"PluginMetaData:<this_ns>/<this_name>"`。先 `clear_refs_from` 本资源再重新注册，防止 spec 更新后旧 ref 残留

### on_change

- 通过 `plugin_metadata_refs`（`PluginMetaDataRefManager`）查找引用此 PluginMetaData 的资源并 requeue。消费方包括：
  - EdgionPlugins（通过 `serviceRegionRouteRef` → ServiceRegionRoute PM → `clusterRef` → ClusterRegionRoute PM 的链）
  - EdgionStreamPlugins（通过 `GlobalConnectionIpRestriction.pluginMetadataRef` → GCIR PM）
  - **其它 PluginMetaData**（ServiceRegionRoute → ClusterRegionRoute，新增的 PM→PM 反向 requeue）
- requeue 通过 `ep_key.split_once(':')` 按 `<Kind>:<ns>/<name>` 约定分发到对应 processor

### on_delete

- 从 `PluginMetaDataStore` 移除
- 从 `RegionRouteClusterTable` / `RegionRouteServiceTable` 移除
- `clear_refs_from` 清除本资源注册的 cross-PM 出向引用
- Requeue 所有依赖此 PM 的资源（让它们的 `ResolvedRefs` 重新计算为 False）

### update_status

PluginMetaData 的 status 遵循 Gateway API 约定，写两个 condition（使用 `k8s_openapi::Condition` 类型）：

| Condition | 语义 | 驱动 |
|-----------|------|------|
| `Accepted` | spec 是否通过 preparse 校验 | preparse 返回的 validation_errors；空 → `True`/`Accepted`，非空 → `False`/`Invalid` |
| `ResolvedRefs` | 外部引用是否解析成功 | 仅 `ServiceRegionRoute.clusterRef` 有意义；其他变体恒 `True`。找不到或目标类型不是 ClusterRegionRoute → `False`/`InvalidPluginMetaDataRef`（`ResolvedRefsError::PluginMetaDataNotFound` 变体） |

`last_transition_time` 仅在 status 值变化时刷新。

## EdgionPlugins 侧验证逻辑

Controller **不会**将路由配置复制进 EdgionPlugins，也**不会**自动创建 ServiceRegionRoute PluginMetaData。EdgionPlugins handler 做一次**整链**校验（`serviceRegionRouteRef` → ServiceRegionRoute PM → `clusterRef` → ClusterRegionRoute PM），结果写入 `EdgionPlugins.status.conditions[Accepted]`：

1. 检查 `serviceRegionRouteRef` 是否设置（未设置则记录 validation error）
2. 从 `PluginMetaDataStore` 查找 ServiceRegionRoute PluginMetaData（未找到则记录）
3. 检查找到的 PM 是否包含 `ServiceRegionRoute` 变体，以及其 `clusterRef` 能否解析到一个 ClusterRegionRoute PM

与此同时，PluginMetaData 自己的 status 也会独立反映**直接**引用的解析情况（`ServiceRegionRoute.clusterRef` → ClusterRegionRoute PM，见上文 update_status 表）。这是一个**健康的双层重复**：
- PM 层 `ResolvedRefs` 回答"这个 ServiceRegionRoute PM 自己的直接引用能否解析"
- EP 层 `Accepted` 回答"整条 EP → PM → PM 链能否打通"

**ServiceRegionRoute PluginMetaData 必须由管理员预先创建**，Controller 不会自动生成。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| ServiceRegionRoute PluginMetaData → ClusterRegionRoute PluginMetaData | PluginMetaData | `clusterRef` 字段 | Service PM 链接集群 PM；PM 自己的 `ResolvedRefs` condition 反映解析结果 |
| PluginMetaData ← EdgionPlugins | EdgionPlugins | `serviceRegionRouteRef` | EP 引用 ServiceRegionRoute PM |
| PluginMetaData ← EdgionStreamPlugins | EdgionStreamPlugins | `GlobalConnectionIpRestriction.pluginMetadataRef` | ESP 引用 GCIR PM，ESP 自己的 `ResolvedRefs` 反映解析结果 |
| PluginMetaData → {EdgionPlugins, EdgionStreamPlugins, PluginMetaData} | 多种 | `plugin_metadata_refs` | PM 变更触发消费方 requeue；ClusterRegionRoute PM 变更会 requeue 依赖它的 ServiceRegionRoute PM（新增的 PM→PM 反向 requeue） |
