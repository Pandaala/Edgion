# RegionRoute 插件

> **Edgion 扩展**
>
> RegionRoute 是 `EdgionPlugins` CRD 提供的多区域流量路由插件，不属于标准 Gateway API。

## 概述

RegionRoute 通过有序路由规则（routeRules）将流量路由到多个 region，适用于同一服务在多个 region 主主部署、需要按
account/user 等维度做确定性粘性路由的场景。

**路由由 `routeRules` 驱动——按顺序评估，第一个匹配规则生效，所有规则未命中则放行。**

三种规则类型：

| 规则 | 行为 |
|------|------|
| `RouteByDirectRegion` | 从请求中提取 region 名称，直接路由 |
| `RouteByKeyConfMatch` | 将提取的 key 与固定 key→region 映射表匹配 |
| `RouteByHashRange` | 一致性哈希范围路由（0 到 modulo-1）；**终止性**——停止后续规则评估 |

**核心行为：**
- 只改变上游 **IP**（通过 DNS 解析 `backendEndpoint`）。原始请求的 **Host header**、**SNI**、路径、body 全部保持不变。
- 每个 region 占用 `[0, modulo-1]` 中的一段范围，范围不得重叠，允许有间隙。
- `myRegion`：若路由结果等于本网关所在 region，插件直接放行，不做跨 region 跳转。
- `failoverTo`：per-region 静态重定向——该 region 的流量重定向到另一个 region。
- `routeRules` 为空 → 插件 no-op。

## 架构：Base + Overlay 模型

RegionRoute 配置遵循 **EdgionConfigData overlay 模型**：

| 层次 | 资源 | 管理者 | 说明 |
|------|------|--------|------|
| Base（基础配置） | `EdgionPlugins` RegionRoute 配置 | git/operator | 路由逻辑 + 默认 region 表，始终存在且安全 |
| Overlay（覆盖层，可选） | `RegionRouteOverride` `EdgionConfigData` | operator / Center | 热更新 region 表；应用时整体替换 base 中的 region 表 |

Base 配置（`EdgionPlugins`）包含所有路由逻辑**和**一个默认 `regions` 表。可选的 `overrideRef` 指向一个
`RegionRouteOverride` 类型的 `EdgionConfigData` 对象，用于动态热更新 region 表。

**Fail-to-base（降级到 Base）：** 若 overlay 不存在、无效或类型不匹配，网关自动使用 base 中的默认 `regions`
表——流量持续正常，不会因 overlay 缺失而中断。

## 配置参数

### EdgionPlugins RegionRouteConfig（Base）

所有路由逻辑在此配置，`overrideRef` 为可选项。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `myRegion` | String | `""` | 本网关所在 region；路由结果等于此值时直接放行 |
| `regions` | Array | `[]` | 默认 region 定义表（overlay 不存在或失败时使用） |
| `keyGet` | Array | `[]` | 公共 key 提取配置 |
| `hashKeyGet` | Array | 无 | 预计算哈希源（RouteByHashRange 优先使用） |
| `hashCalc` | Object | 无 | 哈希计算配置；缺省则 RouteByHashRange 仅透传 |
| `routeRules` | Array | `[]` | 有序路由规则列表；空列表 = 插件 no-op |
| `routeByKeyConfMatch` | Object | 无 | RouteByKeyConfMatch matchMap 配置 |
| `overrideRef` | Object | 无 | 可选，指向 `RegionRouteOverride` EdgionConfigData |

### `regions[]`（RegionDef）子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | 必填 | Region 标识名 |
| `hashRange` | Array\<Integer\> | 必填 | 哈希范围 `[low, high]`，闭区间 |
| `backendEndpoint` | String | 必填 | 后端端点，格式 `host` 或 `host:port` |
| `tls` | Boolean | `true` | 上游连接是否使用 TLS |
| `failoverTo` | String | 无 | 静态故障转移目标 region 名 |

### `hashCalc` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `algorithm` | Enum | `crc32` | 哈希算法：`crc32` 或 `sha256` |
| `modulo` | Integer | `1000` | 哈希值域 `[0, modulo-1]`，必须 >= 2 |
| `writeBack` | Array | `[]` | 将计算出的哈希写入的目标 |

### EdgionConfigData RegionRouteOverride 配置（Overlay）

所有字段均为可选，只有 overlay 中存在的字段才会替换 base 中的对应值。

| 参数 | 类型 | 说明 |
|------|------|------|
| `regions` | Array | 整体替换 region 表（不做合并，整表替换） |

---

## 快速开始

### 第一步：在 EdgionPlugins 中创建 Base 配置

所有逻辑和默认 region 表均配置在 `EdgionPlugins` 中：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-app-plugins
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        myRegion: virginia

        # 默认 region 表（overlay 不存在或失败时使用）
        regions:
          - name: virginia
            hashRange: [0, 599]
            backendEndpoint: virginia-internal.example.com:8443
            tls: true
          - name: ohio
            hashRange: [600, 999]
            backendEndpoint: ohio-internal.example.com:8443
            tls: true

        keyGet:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id

        hashCalc:
          algorithm: crc32
          modulo: 1000
          writeBack:
            - type: responseHeader
              name: X-Region-Hash

        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Direct-Region
          - type: RouteByKeyConfMatch
          - type: RouteByHashRange

        routeByKeyConfMatch:
          matchMap:
            "vip-001": virginia
            "enterprise-xyz": ohio

        # 可选：指向动态 region 表 overlay
        overrideRef:
          name: region-overlay
          namespace: edgion-system
```

### 第二步（可选）：创建 RegionRouteOverride EdgionConfigData

创建一个 `type: RegionRouteOverride` 的 EdgionConfigData，用于在不修改 `EdgionPlugins` 配置的情况下热更新 region 表：

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: region-overlay
  namespace: edgion-system
spec:
  enable: true
  data:
    type: RegionRouteOverride
    config:
      regions:                # 整体替换 base 中的 region 表
        - name: virginia
          hashRange: [0, 499]
          backendEndpoint: virginia-v2.example.com:8443
          tls: true
        - name: ohio
          hashRange: [500, 999]
          backendEndpoint: ohio-v2.example.com:8443
          tls: true
          failoverTo: virginia
```

更新此对象后，所有引用它的网关将立即热更新 region 表，无需重新部署 `EdgionPlugins`。
删除此对象后，网关自动回退到 base 中的默认 region 表。

---

## Region 哈希范围

`hashCalc.modulo`（默认 **1000**）定义哈希值域 `[0, modulo-1]`。所有 region 的 `hashRange[1]` 必须满足 `< modulo`。

| 流量比例 | virginia 范围 | ohio 范围 |
|----------|--------------|----------|
| 50 / 50 | `[0, 499]` | `[500, 999]` |
| 60 / 40 | `[0, 599]` | `[600, 999]` |
| 80 / 20 | `[0, 799]` | `[800, 999]` |
| 金丝雀（ohio=5%） | `[0, 949]` | `[950, 999]` |

允许有间隙。哈希值落入间隙的请求直接透传到正常上游。

## RouteByHashRange 详解

1. 读取 `hashKeyGet`——若值能解析为 `[0, modulo)` 内的整数，直接使用。
2. 通过 `hashCalc` 计算——从 `keyGet` 提取 key，计算 `crc32(key) % modulo`。
3. 将计算结果写入 `hashCalc.writeBack` 指定的目标。

`RouteByHashRange` 是**终止性**规则：透传时停止后续规则评估。

后端服务可以预计算哈希并存入 Cookie，供后续请求复用：

```python
import zlib
region_hash = zlib.crc32(account_id.encode()) % 1000  # modulo 须与配置一致
# Set-Cookie: x_region_hash=<region_hash>
```

## Access Log

插件将条目写入请求的 plugin log：

| 条目 | 含义 |
|------|------|
| `direct=ohio(from:hdr:X-Direct-Region)` | 通过直接路由规则路由 |
| `direct unknown 'foo', fallthrough` | 未知 region 名，继续下一规则 |
| `match=hdr:X-Account-Id→virginia` | key 命中 matchMap，路由到 virginia |
| `match=hdr:X-Account-Id no conf` | key 未命中 matchMap，继续下一规则 |
| `hash_pass=342 from:cookie:x_region_hash` | 使用 Cookie 中的预计算哈希 |
| `key=hdr:X-Account-Id hash=342` | 从 header 计算哈希 |
| `redirect ohio→virginia` | 触发静态 failoverTo 重定向 |
| `local, pass` | 路由目标等于 myRegion，直接放行 |
| `err: failover to invalid` | failoverTo 目标配置错误 |
| `→virginia 127.0.0.1:30001` | 跳转目标（region，host:port） |
| `key missing, pass` | 未找到 key，直接放行 |
| `overlay_missing` | overrideRef 已配置但 EdgionConfigData 不存在；使用 base |
| `overlay_type_mismatch` | EdgionConfigData 类型不符合 RegionRouteOverride；使用 base |

## myRegion 与 failoverTo

### myRegion — 本地直通

在 region 内部署网关时，将 `myRegion` 设置为该 region 名。当路由结果等于本地 region 时，插件跳过
`ExternalJumpPreset` 直接放行——无需跨 region 网络跳转。

### failoverTo — 静态故障转移

在任意 `RegionDef` 中添加 `failoverTo`，将该 region 的流量静态重定向到另一个 region。可在 base `regions`
表或 `RegionRouteOverride` overlay 中设置。典型用途：将某 region 标记为下线，流量自动重定向。

限制：目标必须是有效的 region 名；不支持链式重定向；不支持自引用或循环（运行时静默忽略，access log 中记录 `err: failover to invalid`）。

## 注意事项

- `backendEndpoint` 在每次请求时进行 DNS 解析（Pingora `ExternalJumpPreset`）。建议使用稳定的内部 DNS 名称。
- 插件**不修改** Host header 或 SNI。
- `routeRules: []`（空列表）将使插件变为 no-op，适用于分阶段上线。
- `RegionRouteOverride` EdgionConfigData 可在任意命名空间（通过 `overrideRef.namespace` 指定）。

## 相关文档

- [HTTPRoute 概述](../http-route/overview.md)
- [负载均衡算法](../http-route/lb-algorithms.md)
