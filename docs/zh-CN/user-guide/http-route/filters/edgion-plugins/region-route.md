# RegionRoute 插件

> **🔌 Edgion 扩展**
> 
> RegionRoute 是 `EdgionPlugins` CRD 提供的多区域流量路由插件，不属于标准 Gateway API。

## 概述

RegionRoute 使用有序路由规则将流量分发到多个区域（集群）：直接区域覆盖、key 到区域的映射，以及带可选故障转移的一致性哈希范围路由。

在 `EdgionPlugins` CR 中，插件配置只持有一个 `serviceRegionRouteRef`。所有路由规则——集群、哈希范围和故障转移状态——均位于 `EdgionConfigData` 资源中。插件构建时，Gateway 解析以下链路：

```text
EdgionPlugins.serviceRegionRouteRef
  → ServiceRegionRoute（EdgionConfigData）
    → ServiceRegionRoute.clusterRef → ClusterRegionRoute（EdgionConfigData）
```

`ServiceRegionRoute` 和 `ClusterRegionRoute` EdgionConfigData 资源必须由管理员预先创建（`ClusterRegionRoute` 通常由 Center 管理，Center 同时写入各集群的故障转移状态）。

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
        # 引用同命名空间下的 ServiceRegionRoute EdgionConfigData
        serviceRegionRouteRef: my-svc-region-route
```

---

## 配置参数

### 插件配置（EdgionPlugins）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `serviceRegionRouteRef` | String | 无 | 与 `EdgionPlugins` 资源同命名空间的 `ServiceRegionRoute` EdgionConfigData 名称。所有路由规则均从此引用解析 |

`dye` 是**条目级（entry-level）**字段——与 `type`/`config`/`enable` 同级，不属于 `config` 的成员：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `dye` | Object | 无 | 按结果分类的流量染色请求头/响应头（`on: [success, fallback]`——RegionRoute 没有 `reject` 能力，见下文）。`request`/`response` 为 `{name, on, value, conditions}` 条目列表；不设置 `value` 时，`success` 上携带最终区域名（`failoverTo` 重定向之后的区域）。完整参考见 [染色（Dye）](./dye.md)，插件特有内容见下方[染色请求头](#染色请求头)。 |

插件除共享的 `dye` 校验外不进行其他本地字段校验——被引用的 EdgionConfigData 是否存在由 Controller 的 `preparse()`（通过 status conditions 暴露）和 Gateway 在插件构建时检查。

### 路由模型（EdgionConfigData）

被引用的 EdgionConfigData 定义路由规则，按**顺序求值——先匹配者优先**：

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

EdgionConfigData 使用 `RouteByKeyConfMatch` 规则，将请求头的值映射到区域，使命名租户始终落在同一集群。`EdgionPlugins` 配置保持最简：

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

EdgionConfigData 定义 `RouteByHashRange`，为各集群配置 `hashRange` 切片（例如 50/50 分配为 `0–499` 和 `500–999`）。请求的计算哈希值落在某集群范围内时，路由到该集群。因为 `RouteByHashRange` 是终止性的，请将其放在最后。

### 3. 区域故障转移

在 `ClusterRegionRoute` EdgionConfigData 中为集群设置 `failoverTo`（通常由 Center 驱动）。目标为降级集群的流量将被透明地重定向到指定的健康集群——`EdgionPlugins` 和 HTTPRoute 无需改变。

---

## 染色请求头

完整参考见 [染色（Dye）](./dye.md)：`on` 结果词表、两种 `value` 来源、模板引擎、防伪造剥离（即使插件被 `enable: false` 关闭或被 conditions 跳过也会执行）、请求头名校验，以及安全指引。本节只覆盖 RegionRoute 特有的部分。

`dye` 默认关闭：不配置时插件不设置任何请求头，也不做任何剥离。配置后，会在请求上（或在 `response` 方向规则的响应上）附加运维自定义的 HTTP 头：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      dye:
        request:
          - name: X-Served-Region     # 不设置 value => 最终区域名（默认 on: [success]）
        response:
          - name: X-Edgion-Outcome
            on: [fallback]            # 不设置 value => region_route:fallback:overlay-fail-to-base
      config:
        serviceRegionRouteRef: my-svc-region-route
```

- `request` 条目设置在转发给上游的请求上（对于本地区域直通场景，则设置在继续经过网关自身处理链的请求上）；`response` 条目设置在返回给客户端的响应上。
- `success` 上不设置 `value` 时携带**最终区域名**——即经过 `failoverTo` 重定向之后的区域，而不是最初匹配到的区域。
- `success` 染色适用于任何一条规则（`RouteByDirectRegion`、`RouteByKeyConfMatch`，或终止性的 `RouteByHashRange`）实际选中区域的情形，包括**本地区域直通**场景——当解析出的区域正是本网关自身所在区域时，不会发生跨区域跳转，但请求仍然被路由到了一个区域，因此仍会以该区域名染色。
- RegionRoute 只支持 `success` 和 `fallback` 两种染色能力——**不支持** `reject`：它唯一的失败类分支是 overlay fail-to-base 路径（`RegionRouteOverride` 解析失败后回落到基础路由规则），会以 `fallback`、cause 为 `overlay-fail-to-base` 记录。当其他插件已经锁定了上游跳转，或没有任何规则匹配时，不会染色——这两种情况目前都不是已记录的结果。`reject` 能力将在 `SetJumpOutcome::Rejected` 静默放行这个已知的历史 bug 修复后才会开放（该 bug 单独跟踪，详见设计文档）。

---

## 常见问题

### Q1：为什么 EdgionPlugins 资源中没有路由配置？

**A**：这是设计如此。`EdgionPlugins` 配置只持有 `serviceRegionRouteRef`。将集群、哈希范围和故障转移状态保存在 EdgionConfigData 中，允许运维人员（和 Center）集中更新路由而无需编辑每条路由，同时允许多条路由共享同一个 `ServiceRegionRoute`。

### Q2：规则的求值顺序是什么？

**A**：规则从上到下逐条求值，先匹配者优先。`RouteByDirectRegion` 和 `RouteByKeyConfMatch` 不匹配时向后穿透，因此更具体的规则应放在前面。`RouteByHashRange` 是终止性的，应放在最后。

### Q3：路由总是回退，但我配置了 RegionRoute，为什么？

**A**：最常见的原因是 EdgionConfigData 链（`ServiceRegionRoute` → `ClusterRegionRoute`）缺失或无法解析。请检查 HTTPRoute / EdgionPlugins 的 status conditions 和网关日志——Controller 会在那里报告缺失的引用。

---

## 相关文档

- [Dynamic Upstream](./dynamic-upstream.md) — 请求驱动的上游选择
- [Direct Endpoint](./direct-endpoint.md) — 绕过负载均衡直连指定端点
- [过滤器总览](../overview.md) — 所有 HTTPRoute 过滤器
