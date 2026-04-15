---
name: resource-plugin-metadata
description: PluginMetaData 资源：插件体系（类 EdgionPlugins），支持 Global RegionConf + Service 级状态管理。
---

# PluginMetaData 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

PluginMetaData 是 Edgion 的自定义扩展资源，支持两种用途：

1. **数据列表**（StringList / IpList / RegexList）：插件共享的静态数据
2. **Plugin 体系**（`spec.plugins` 数组）：类 EdgionPlugins 的扩展配置，当前支持 RegionRoute，未来可扩展 StreamRegionRoute 等

## 源码位置

- 类型定义: `src/types/resources/plugin_metadata/mod.rs`（主模块）、`src/types/resources/plugin_metadata/region_route.rs`（RegionRoute 配置结构体）
- Plugin 类型: `MetaDataPlugin` enum（ClusterRegionRoute / ServiceRegionRoute 变体）
- Controller Handler: `handlers/plugin_metadata.rs`
- EdgionPlugins Handler（验证逻辑）: `handlers/edgion_plugins.rs`
- PluginMetaDataStore（Controller 侧）: `plugin_metadata_utils/plugin_metadata_store.rs`
- PluginMetaDataGatewayStore（Gateway 侧）: `src/core/gateway/plugins/runtime/plugin_metadata/conf_handler.rs`
- RegionRoute 表（Controller）: `region_route_table/cluster_table.rs`、`region_route_table/service_table.rs`
- Controller API: `src/core/controller/api/region_route_handlers.rs`
- Center API: `src/core/center/api/region_pm_handlers.rs`

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

### MetaDataPlugin 枚举

| 变体 | 用途 |
|------|------|
| `ClusterRegionRoute` | 集群拓扑 + 路由规则 + matchMap（运维/Center 管理） |
| `ServiceRegionRoute` | 服务级 failover + clusterRef 引用（管理员预建） |

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

| 操作 | 目标 | 端点 |
|------|------|------|
| 集群 region failover（静态） | ClusterRegionRoute PluginMetaData `regions[].failoverTo` | `POST /api/v1/center/cluster-region-pms/failover` |
| 服务级 failover | ServiceRegionRoute PluginMetaData `regions[].failoverTo` | `POST /api/v1/center/service-region-pms/failover` |
| 集群 matchMap | ClusterRegionRoute PluginMetaData `routeByKeyConfMatch.matchMap` | 通过 proxy PUT PluginMetaData |

### Controller API

| 端点 | 说明 |
|------|------|
| `GET /api/v1/cluster-region-pms` | 列出所有 ClusterRegionRoute PluginMetaData |
| `GET /api/v1/service-region-pms` | 列出所有 ServiceRegionRoute PluginMetaData |
| `POST /api/v1/cluster-region-pms/failover` | 修改 Cluster PM 中某 region 的 failoverTo |
| `POST /api/v1/service-region-pms/failover` | 修改 Service PM 中某 region 的 failoverTo |

## Controller 侧处理

### parse

- 更新 `PluginMetaDataStore`（供 EdgionPlugins handler 验证时查找）
- 更新 `RegionRouteClusterTable`（ClusterRegionRoute 变体）或 `RegionRouteServiceTable`（ServiceRegionRoute 变体）

### on_change

- 通过 `plugin_metadata_refs`（`PluginMetaDataRefManager`）查找引用此 PluginMetaData 的 EdgionPlugins → requeue（用于更新 status conditions；Gateway 不依赖此 requeue 即可正确路由）

### on_delete

- 从 `PluginMetaDataStore` 移除
- 从 `RegionRouteClusterTable` / `RegionRouteServiceTable` 移除
- Requeue 依赖资源

## EdgionPlugins 侧验证逻辑

Controller **不会**将路由配置复制进 EdgionPlugins，也**不会**自动创建 ServiceRegionRoute PluginMetaData。它只做存在性校验，验证结果写入 status conditions：

1. 检查 `serviceRegionRouteRef` 是否设置（未设置则在 `validation_error` 中写入错误）
2. 从 `PluginMetaDataStore` 查找 ServiceRegionRoute PluginMetaData（未找到则写入 `validation_error`）
3. 检查找到的 PluginMetaData 是否包含 `ServiceRegionRoute` 变体，以及其 `clusterRef` 是否存在

`validation_error` 是 `RegionRouteConfig` 上的 `#[serde(skip)]` 字段，仅在 Controller 内存中存在，不写入 K8s 资源。错误会体现在 `EdgionPlugins.status.conditions[Accepted]` 中。

**ServiceRegionRoute PluginMetaData 必须由管理员预先创建**，Controller 不会自动生成。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| ServiceRegionRoute PluginMetaData → ClusterRegionRoute PluginMetaData | PluginMetaData | `clusterRef` 字段 | Service PluginMetaData 链接集群 PluginMetaData |
| PluginMetaData ← EdgionPlugins | EdgionPlugins | `serviceRegionRouteRef` | EP 引用 ServiceRegionRoute PluginMetaData |
| PluginMetaData → EdgionPlugins | EdgionPlugins | `plugin_metadata_refs` | PluginMetaData 变更触发 EP requeue |
