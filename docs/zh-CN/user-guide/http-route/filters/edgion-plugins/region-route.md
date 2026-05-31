# RegionRoute 插件

> **🔌 Edgion 扩展**
> 
> RegionRoute 是 `EdgionPlugins` CRD 提供的多区域流量路由插件，不属于标准 Gateway API。

## 概述

RegionRoute 使用有序路由规则将流量分发到多个区域（集群）：直接区域覆盖、key 到区域的映射，以及带可选故障转移的一致性哈希范围路由。

在 `EdgionPlugins` CR 中，插件配置只持有一个 `serviceRegionRouteRef`。所有路由规则——集群、哈希范围和故障转移状态——均位于 `PluginMetaData` 资源中。插件构建时，Gateway 解析以下链路：

```text
EdgionPlugins.serviceRegionRouteRef
  → ServiceRegionRoute（PluginMetaData）
    → ServiceRegionRoute.clusterRef → ClusterRegionRoute（PluginMetaData）
```

`ServiceRegionRoute` 和 `ClusterRegionRoute` PluginMetaData 资源必须由管理员预先创建（`ClusterRegionRoute` 通常由 Center 管理，Center 同时写入各集群的故障转移状态）。

## 快速开始

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-region-route-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        # 引用同命名空间下的 ServiceRegionRoute PluginMetaData
        serviceRegionRouteRef: my-svc-region-route
```

---

## 配置参数

### 插件配置（EdgionPlugins）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `serviceRegionRouteRef` | String | 无 | 与 `EdgionPlugins` 资源同命名空间的 `ServiceRegionRoute` PluginMetaData 名称。所有路由规则均从此引用解析 |

插件不进行本地字段校验——被引用的 PluginMetaData 是否存在由 Controller 的 `preparse()`（通过 status conditions 暴露）和 Gateway 在插件构建时检查。

### 路由模型（PluginMetaData）

被引用的 PluginMetaData 定义路由规则，按**顺序求值——先匹配者优先**：

| 规则类型 | 行为 |
|----------|------|
| `RouteByDirectRegion` | 直接从请求中提取区域名称（通过 `keyFrom` 来源，按顺序）；使用第一个匹配已定义区域的值。不匹配时向后穿透 |
| `RouteByKeyConfMatch` | 提取 key 并在固定 `matchMap`（key → 区域）中查找。匹配时路由到映射的区域；否则向后穿透 |
| `RouteByHashRange` | 一致性哈希范围路由。若存在有效的预计算哈希（在 `[0, modulo)` 范围内）则使用，否则重新计算。此规则**终止性**——不会穿透到后续规则 |

每个区域（集群）定义：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | — | 集群标识（用于直接区域匹配和故障转移） |
| `hashRange` | `[Integer, Integer]` | — | 闭区间哈希范围 `[low, high]`；须满足 `0 ≤ low ≤ high < modulo` |
| `backendEndpoint` | String | — | 目标端点 `host` 或 `host:port`。仅更改上游 IP；原始 SNI 和请求头保留不变 |
| `tls` | Boolean | `true` | 上游连接是否使用 TLS（`true` 时端口默认 `443`，`false` 时默认 `80`） |
| `failoverTo` | String | 无 | 将该集群的流量重定向到指定集群；必须是同一列表中的有效集群，且不能指向自身 |

哈希计算：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `algorithm` | Enum | `Crc32` | 哈希算法：`Crc32`（快速，硬件加速）或 `Sha256` |
| `modulo` | Integer | `1000` | 哈希值域 `[0, modulo-1]`；所有区域的 `hashRange[1]` 必须 `< modulo` |
| `writeBack` | Array | 无 | 将计算出的哈希写回的目标（空 = 不写回） |

---

## 常见配置场景

### 1. 将特定租户固定到区域（直接映射）

PluginMetaData 使用 `RouteByKeyConfMatch` 规则，将请求头的值映射到区域，使命名租户始终落在同一集群。`EdgionPlugins` 配置保持最简：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-region-route
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        serviceRegionRouteRef: tenant-region-route-meta
```

### 2. 一致性哈希范围分配

PluginMetaData 定义 `RouteByHashRange`，为各集群配置 `hashRange` 切片（例如 50/50 分配为 `0–499` 和 `500–999`）。请求的计算哈希值落在某集群范围内时，路由到该集群。因为 `RouteByHashRange` 是终止性的，请将其放在最后。

### 3. 区域故障转移

在 `ClusterRegionRoute` PluginMetaData 中为集群设置 `failoverTo`（通常由 Center 驱动）。目标为降级集群的流量将被透明地重定向到指定的健康集群——`EdgionPlugins` 和 HTTPRoute 无需改变。

---

## 常见问题

### Q1：为什么 EdgionPlugins 资源中没有路由配置？

**A**：这是设计如此。`EdgionPlugins` 配置只持有 `serviceRegionRouteRef`。将集群、哈希范围和故障转移状态保存在 PluginMetaData 中，允许运维人员（和 Center）集中更新路由而无需编辑每条路由，同时允许多条路由共享同一个 `ServiceRegionRoute`。

### Q2：规则的求值顺序是什么？

**A**：规则从上到下逐条求值，先匹配者优先。`RouteByDirectRegion` 和 `RouteByKeyConfMatch` 不匹配时向后穿透，因此更具体的规则应放在前面。`RouteByHashRange` 是终止性的，应放在最后。

### Q3：路由总是回退，但我配置了 RegionRoute，为什么？

**A**：最常见的原因是 PluginMetaData 链（`ServiceRegionRoute` → `ClusterRegionRoute`）缺失或无法解析。请检查 HTTPRoute / EdgionPlugins 的 status conditions 和网关日志——Controller 会在那里报告缺失的引用。

---

## 相关文档

- [Dynamic Upstream](./dynamic-upstream.md) — 请求驱动的上游选择
- [Direct Endpoint](./direct-endpoint.md) — 绕过负载均衡直连指定端点
- [过滤器总览](../overview.md) — 所有 HTTPRoute 过滤器
