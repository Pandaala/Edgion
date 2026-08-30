# RegionRoute 插件

> **🔌 Edgion 扩展**
> 
> RegionRoute 是 `EdgionPlugins` CRD 提供的多区域流量路由插件，不属于标准 Gateway API。

## 概述

RegionRoute 使用有序路由规则将流量分发到多个区域（集群）：直接区域覆盖、key 到区域的映射，以及带可选故障转移的一致性哈希范围路由。

所有路由规则——路由规则本身、key 提取、哈希计算——**以及**默认区域表都位于 `EdgionPlugins` 的插件配置内部。该配置是自足的：一个不引用任何外部资源的 RegionRoute 插件本身就是完整可用的配置。

可选的 `overrideRef` 指向一个 `type: RegionRouteOverride` 的 `EdgionConfigData`。它是一个**纯数据覆盖层**：可以替换区域表，但永远无法改变路由逻辑。Gateway 在**请求时**解析该覆盖层，因此运维人员（或 Center）可以在不改动 `EdgionPlugins` 资源的前提下替换区域表：

```text
EdgionPlugins.requestPlugins[].config.overrideRef
  → EdgionConfigData（spec.data.type: RegionRouteOverride）
      regions?      # 仅数据——绝不包含路由规则
```

若覆盖层缺失、被禁用、类型不符，或合并后得到的区域表非法，插件将使用 `EdgionPlugins` 中的基础 `regions`——**fail-to-base**，绝不 fail-closed。详见[覆盖层解析与 fail-to-base](#覆盖层解析与-fail-to-base)。

> **安全不变式**：由于任何覆盖层失败都会回退到它，`EdgionPlugins` 中的基础 `regions` 必须按**安全默认值**编写。

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
        myRegion: us-east
        keyGet:
          - type: header
            name: X-Tenant-Id
        routeRules:
          - type: RouteByKeyConfMatch
            matchMap:
              tenant-a: us-east
              tenant-b: eu-west
          - type: RouteByHashRange
        hashCalc:
          algorithm: crc32
          modulo: 1000
        # 基础区域表——任何覆盖层失败都会回退到它，必须是安全默认值
        regions:
          - name: us-east
            hashRange: [0, 499]
            backendEndpoint: api.us-east.example.com
          - name: eu-west
            hashRange: [500, 999]
            backendEndpoint: api.eu-west.example.com
        # 可选：可热替换的区域表
        overrideRef:
          name: my-region-override
```

---

## 配置参数

### 插件配置（EdgionPlugins）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `myRegion` | String | `""` | 本 Gateway 实例自身所属的区域名。当规则解析出的区域等于该值时，请求继续走本 Gateway 自身的插件链，而不做跨区域跳转 |
| `keyGet` | Array | `[]` | 供 `RouteByKeyConfMatch` 与 `RouteByHashRange` 共用的 key 提取来源，按顺序取第一个非空值 |
| `hashKeyGet` | Array | 无 | 预计算哈希的读取来源，在计算哈希之前先尝试 |
| `hashCalc` | Object | 无 | 哈希计算配置——见[哈希计算](#哈希计算) |
| `routeRules` | Array | `[]` | 有序路由规则，自上而下求值，先匹配者优先——见[路由规则](#路由规则) |
| `routeByKeyConfMatch` | Object | 无 | `RouteByKeyConfMatch` 的集群级覆盖：`matchMap`（key → 区域）与 `bypassClusterKeys`（列入其中的 key 忽略该覆盖，回退到规则自带的 `matchMap`） |
| `regions` | Array | `[]` | **基础区域表，即安全默认值。** 未配置覆盖层时直接使用，覆盖层任何失败时也回退到它——见[区域定义](#区域定义) |
| `overrideRef` | Object | 无 | `{ name, namespace? }`，指向一个 `type: RegionRouteOverride` 的 `EdgionConfigData`。省略 `namespace` 表示 `EdgionPlugins` 资源自身的命名空间。跨命名空间引用需要相应的 `ReferenceGrant`，或目标资源标记 `visibility: Cluster` |

`dye` 是**条目级（entry-level）**字段——与 `type`/`config`/`enable` 同级，不属于 `config` 的成员：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `dye` | Object | 无 | 按结果分类的流量染色请求头/响应头（`on: [success, fallback]`——RegionRoute 没有 `reject` 能力，见下文）。`request`/`response` 为 `{name, on, value, conditions}` 条目列表；不设置 `value` 时，`success` 上携带最终区域名（`failoverTo` 重定向之后的区域）。完整参考见 [染色（Dye）](./dye.md)，插件特有内容见下方[染色请求头](#染色请求头)。 |

插件除共享的 `dye` 校验外不进行其他本地字段校验。`overrideRef` 能否解析由 Controller 检查，并反映在 `EdgionPlugins` 资源的 `ResolvedRefs` status condition 上；区域表本身则在 Gateway 应用覆盖层时校验。

### 路由规则

按**顺序求值——先匹配者优先**：

| 规则类型 | 行为 |
|----------|------|
| `RouteByDirectRegion` | 通过该规则自带的 `keyFrom` 来源按顺序直接从请求中提取区域名称；使用第一个能在生效区域表中匹配到的值。不匹配时向后穿透 |
| `RouteByKeyConfMatch` | 通过共享的 `keyGet` 提取 key，并在 `matchMap`（key → 区域）中查找。集群级 `routeByKeyConfMatch.matchMap` 非空时优先生效，除非提取到的 key 列在 `bypassClusterKeys` 中——此时使用规则自带的 `matchMap`。匹配时路由到映射的区域；否则向后穿透 |
| `RouteByHashRange` | 一致性哈希范围路由。先尝试从 `hashKeyGet` 读取预计算哈希，若存在且有效（在 `[0, modulo)` 范围内）则直接使用，否则通过 `hashCalc` 计算。此规则**终止性**——不会穿透到后续规则 |

### 区域定义

`regions` 中的每个区域（集群）定义：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | — | 集群标识（用于直接区域匹配和故障转移）。必须非空且在表内唯一 |
| `hashRange` | `[Integer, Integer]` | — | 闭区间哈希范围 `[low, high]`；须满足 `0 ≤ low ≤ high < modulo` |
| `backendEndpoint` | String | — | 目标端点 `host` 或 `host:port`。仅更改上游 IP；原始 SNI 和请求头保留不变 |
| `tls` | Boolean | `true` | 上游连接是否使用 TLS（`true` 时端口默认 `443`，`false` 时默认 `80`） |
| `failoverTo` | String | 无 | 将该集群的流量重定向到指定集群；必须是同一列表中的有效集群，且不能指向自身 |

### 哈希计算

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `algorithm` | Enum | `crc32` | 哈希算法：`crc32`（快速，硬件加速）或 `sha256` |
| `modulo` | Integer | `1000` | 哈希值域 `[0, modulo-1]`；所有区域的 `hashRange[1]` 必须 `< modulo` |
| `writeBack` | Array | 无 | 将计算出的哈希写回的目标（空 = 不写回） |

---

## 覆盖层解析与 fail-to-base

### 覆盖层资源

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: my-region-override
  namespace: production
spec:
  enable: true            # 信封级开关
  data:
    type: RegionRouteOverride
    config:
      # 所有字段均为可选；省略的字段保持基础配置的值
      regions:            # 存在时，对基础区域表做整表替换（不做元素级合并）
        - name: us-east
          hashRange: [0, 999]
          backendEndpoint: api.us-east.example.com
        - name: eu-west
          hashRange: [0, 0]
          backendEndpoint: api.eu-west.example.com
          failoverTo: us-east
```

| 覆盖层字段 | 作用 |
|------------|------|
| `regions` | 对基础区域表做**整表替换**。不存在元素级合并——你提供的表就是最终生效的表 |

`regions` 是唯一有实际作用的字段。`RegionRouteOverride` 类型还声明了 `enable` 与 `active`，但 Gateway 目前两者都不消费——设置它们不产生任何效果。若要让某个覆盖层停止生效，请设置 `EdgionConfigData` 的**信封级**开关 `spec.enable: false`（而不是 `spec.data.config.enable`）；此时插件回退到基础 `regions`，见下文。

路由**逻辑**（`routeRules`、`keyGet`、`hashCalc`、`routeByKeyConfMatch`、`myRegion`）刻意不出现在覆盖层中。逻辑只存在于 `EdgionPlugins`，无法被远程改写。

### fail-to-base

覆盖层在每个请求上解析。以下四条失败路径全部回退到基础 `regions`——流量继续放行，绝不拒绝——并且都会记录一次 cause 为 `overlay-fail-to-base` 的 `fallback` 染色结果，同时打点一个 Prometheus 计数器：

| 情形 | 计数器 |
|------|--------|
| 被引用的 `EdgionConfigData` 的 `spec.enable: false` | `overlay_disabled` |
| 被引用的 `EdgionConfigData` 不存在 | `overlay_missing` |
| 资源存在但 `spec.data.type` 不是 `RegionRouteOverride` | `overlay_type_mismatch` |
| 合并后的区域表校验失败（为空、name 或 endpoint 为空、`hashRange` 首尾颠倒、name 重复） | `overlay_invalid` |

---

## 常见配置场景

### 1. 将特定租户固定到区域（直接映射）

使用 `RouteByKeyConfMatch` 规则，将请求头的值映射到区域，使命名租户始终落在同一集群：

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
        keyGet:
          - type: header
            name: X-Tenant-Id
        routeRules:
          - type: RouteByKeyConfMatch
            matchMap:
              tenant-a: us-east
              tenant-b: eu-west
        regions:
          - name: us-east
            hashRange: [0, 499]
            backendEndpoint: api.us-east.example.com
          - name: eu-west
            hashRange: [500, 999]
            backendEndpoint: api.eu-west.example.com
```

### 2. 一致性哈希范围分配

为各区域在 `modulo` 上划分 `hashRange` 切片（例如 50/50 分配为 `0–499` 和 `500–999`），并添加 `RouteByHashRange`。请求的计算哈希值落在某区域范围内时，路由到该区域。因为 `RouteByHashRange` 是终止性的，请将其放在最后。

### 3. 区域故障转移

为某个区域设置 `failoverTo`，发往该区域的流量将被透明地重定向到指定的健康区域——HTTPRoute 无需改变。

两种驱动方式：

- **静态**：直接写在 `EdgionPlugins` 基础 `regions` 的某个区域上。
- **动态**：为插件配置 `overrideRef`，把 `failoverTo` 写入 `RegionRouteOverride` EdgionConfigData。这正是 Center 做多集群故障转移所走的路径，下一个请求即生效，无需改动 `EdgionPlugins`。

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
        regions:
          - name: us-east
            hashRange: [0, 999]
            backendEndpoint: api.us-east.example.com
        overrideRef:
          name: my-region-override
```

- `request` 条目设置在转发给上游的请求上（对于本地区域直通场景，则设置在继续经过网关自身处理链的请求上）；`response` 条目设置在返回给客户端的响应上。
- `success` 上不设置 `value` 时携带**最终区域名**——即经过 `failoverTo` 重定向之后的区域，而不是最初匹配到的区域。
- `success` 染色适用于任何一条规则（`RouteByDirectRegion`、`RouteByKeyConfMatch`，或终止性的 `RouteByHashRange`）实际选中区域的情形，包括**本地区域直通**场景——当解析出的区域正是本网关自身所在区域时，不会发生跨区域跳转，但请求仍然被路由到了一个区域，因此仍会以该区域名染色。
- RegionRoute 只支持 `success` 和 `fallback` 两种染色能力——**不支持** `reject`：它唯一的失败类分支是 overlay fail-to-base 路径（`RegionRouteOverride` 解析失败后回落到基础路由规则），会以 `fallback`、cause 为 `overlay-fail-to-base` 记录。当其他插件已经锁定了上游跳转，或没有任何规则匹配时，不会染色——这两种情况目前都不是已记录的结果。`reject` 能力将在 `SetJumpOutcome::Rejected` 静默放行这个已知的历史 bug 修复后才会开放（该 bug 单独跟踪，详见设计文档）。

---

## 常见问题

### Q1：使用 RegionRoute 必须创建 EdgionConfigData 吗？

**A**：不需要。`EdgionPlugins` 配置本身就是完整的——路由逻辑和基础区域表都在其中。只有当你希望区域表能在运行时替换而不改动 `EdgionPlugins` 时，才需要添加 `overrideRef`；Center 正是用这条路径驱动多集群故障转移。

### Q2：规则的求值顺序是什么？

**A**：规则从上到下逐条求值，先匹配者优先。`RouteByDirectRegion` 和 `RouteByKeyConfMatch` 不匹配时向后穿透，因此更具体的规则应放在前面。`RouteByHashRange` 是终止性的，应放在最后。

### Q3：我更新了 override，但路由没有变化，为什么？

**A**：覆盖层在任何解析问题上都会静默回退到基础 `regions`——这是设计使然（fail-to-base，而非 fail-closed）。请按顺序排查：`EdgionPlugins` 的 `ResolvedRefs` status condition（引用是否解析成功？跨命名空间引用是否被放行？）、被引用资源的 `spec.enable` 与 `spec.data.type`，以及 Gateway 的插件日志——日志会记录命中的是 `overlay_disabled` / `overlay_missing` / `overlay_type_mismatch` / `overlay_invalid` 中的哪一种。

### Q4：override 能修改路由规则吗？

**A**：不能，这是刻意为之。`RegionRouteOverride` 只承载数据，且实际生效的只有 `regions`。路由逻辑只存在于 `EdgionPlugins`，因此远程写入方可以改变流量去向，但无法改写流量的分类方式。

---

## 相关文档

- [Dynamic Upstream](./dynamic-upstream.md) — 请求驱动的上游选择
- [Direct Endpoint](./direct-endpoint.md) — 绕过负载均衡直连指定端点
- [过滤器总览](../overview.md) — 所有 HTTPRoute 过滤器
