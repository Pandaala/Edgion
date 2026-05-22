# RegionRoute 插件

> **🔌 Edgion 扩展**
> 
> RegionRoute 是 `EdgionPlugins` CRD 提供的多区域流量路由插件，不属于标准 Gateway API。

## 概述

RegionRoute 通过有序路由规则（routeRules）将流量路由到多个 region，适用于同一服务在多个 region 主主部署、需要按 account/user 等维度做确定性粘性路由的场景。

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
- `myRegion`：若路由结果等于本网关所在 region，插件直接放行，不设 `ExternalJumpPreset`（流量已在本地，无需跨 region 跳转）。
- `failoverTo`：per-region 静态重定向——该 region 的流量直接重定向到另一个 region。
- `routeRules` 为空 → 插件 no-op。

## 架构概述

RegionRoute 的配置分为三层：

| 层次 | 资源 | 管理者 | 说明 |
|------|------|--------|------|
| 集群配置 | `ClusterRegionRoute` PluginMetaData | 运维/Center | 持有完整路由配置：regions、keyGet、hashCalc、routeRules、routeByKeyConfMatch |
| 服务配置 | `ServiceRegionRoute` PluginMetaData | 管理员预建 | 引用集群配置（`clusterRef`），可按 region 设置 `failoverTo` 覆盖 |
| 插件引用 | `EdgionPlugins` RegionRoute plugin | 用户 | 只有 `serviceRegionRouteRef` 一个字段，指向 Service PM 名称 |

引用链：`EP.serviceRegionRouteRef → ServiceRegionRoute PM → clusterRef → ClusterRegionRoute PM`

**ServiceRegionRoute PluginMetaData 必须由管理员预先创建**，系统不会自动生成。

## 配置参数

### EdgionPlugins 中的 RegionRoute 配置

`EdgionPlugins` 中的 RegionRoute 只有一个字段：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `serviceRegionRouteRef` | String | 无 | 同命名空间下 ServiceRegionRoute PluginMetaData 的名称 |

### ClusterRegionRoute PluginMetaData 配置

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `myRegion` | String | `""` | 本网关所在 region 名称，路由目标等于此值时直接放行 |
| `regions` | Array\<Object\> | `[]` | Region 定义列表，见 `RegionDef` 子字段 |
| `keyGet` | Array\<Object\> | `[]` | 共享的 key 提取配置（RouteByKeyConfMatch 和 RouteByHashRange 共用） |
| `hashKeyGet` | Array\<Object\> | 无 | 预计算 hash 读取源（RouteByHashRange 计算前先尝试读取） |
| `hashCalc` | Object | 无 | Hash 计算配置，缺省时 RouteByHashRange 仅做 passthrough |
| `routeRules` | Array\<Object\> | `[]` | 有序路由规则列表，空列表时插件为 no-op |
| `routeByKeyConfMatch` | Object | 无 | RouteByKeyConfMatch 的 matchMap 配置（由 Center 管理） |

### `regions[]`（RegionDef）子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | —（必填） | Region 标识名称 |
| `hashRange` | Array\<Integer\> | —（必填） | Hash 范围 `[low, high]`，闭区间，须满足 `high < modulo` |
| `backendEndpoint` | String | —（必填） | 后端端点，格式 `host` 或 `host:port` |
| `tls` | Boolean | `true` | 上游连接是否使用 TLS |
| `failoverTo` | String | 无 | 静态 failover 目标 region 名称 |

### ServiceRegionRoute PluginMetaData 配置

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `clusterRef` | Object | 无 | 指向 ClusterRegionRoute PluginMetaData 的引用（namespace + name） |
| `regions` | Array\<Object\> | `[]` | per-region failoverTo 覆盖列表（只有 `name` 和 `failoverTo` 字段有效） |

### `hashCalc` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `algorithm` | Enum | `crc32` | Hash 算法。可选值：`crc32` | `sha256` |
| `modulo` | Integer | `1000` | Hash 值域 `[0, modulo-1]`，须 >= 2 |
| `writeBack` | Array\<Object\> | `[]` | 将计算得到的 hash 写入目标列表 |

---

## 快速开始

### 第一步：创建 ClusterRegionRoute PluginMetaData（运维/Center 操作）

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
        # 本网关所在 region——路由目标等于此值时直接放行
        myRegion: virginia

        # region 定义 — hashRange [low, high]，闭区间
        regions:
          - name: virginia
            hashRange: [0, 599]           # 60% 流量（1000 槽中的 600 个）
            backendEndpoint: virginia-internal.example.com:8443
            tls: true
          - name: ohio
            hashRange: [600, 999]         # 40% 流量（1000 槽中的 400 个）
            backendEndpoint: ohio-internal.example.com:8443
            tls: true

        # RouteByKeyConfMatch 和 RouteByHashRange 共用的 key 提取配置
        keyGet:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id

        # hash 计算配置
        hashCalc:
          algorithm: crc32
          modulo: 1000
          writeBack:
            - type: responseHeader
              name: X-Region-Hash

        # 有序路由规则
        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Direct-Region
          - type: RouteByKeyConfMatch
          - type: RouteByHashRange

        # RouteByKeyConfMatch 映射表（由 Center 管理）
        routeByKeyConfMatch:
          matchMap:
            "vip-001": virginia
            "enterprise-xyz": ohio
```

### 第二步：创建 ServiceRegionRoute PluginMetaData（管理员操作）

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
        # 引用集群配置
        clusterRef:
          name: global-region-conf
          namespace: edgion-system
        # 可选：per-region failoverTo 覆盖
        regions:
          - name: ohio
            failoverTo: virginia     # ohio 流量全量切到 virginia
```

### 第三步：在 EdgionPlugins 中引用

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-app-plugins
  namespace: default
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        # 只有一个字段：同命名空间下的 ServiceRegionRoute PM 名称
        serviceRegionRouteRef: my-svc-pm
```

## Region 哈希范围

`hashCalc.modulo`（默认 **1000**）定义 hash 值域 `[0, modulo-1]`——默认恰好 1000 个槽（0..=999）。所有 region 的 `hashRange` 值必须满足 `hashRange[1] < modulo`。

| 意图 | virginia 范围 | ohio 范围 |
|------|--------------|----------|
| 各 50% | `[0, 499]` | `[500, 999]` |
| 60 / 40 | `[0, 599]` | `[600, 999]` |
| 80 / 20 | `[0, 799]` | `[800, 999]` |
| 灰度（ohio=5%） | `[0, 949]` | `[950, 999]` |

允许有间隙，落在间隙的请求放行到原始上游。

## RouteByHashRange 详细逻辑

`RouteByHashRange` 先尝试 `hashKeyGet`（预计算 passthrough），若无效再回落到 `hashCalc` 计算：

1. 读取 `hashKeyGet` — 若值能解析为 `[0, modulo)` 内的整数，直接使用（日志：`hash_pass=N from:SRC`）。
2. 通过 `hashCalc` 计算 — 从共用 `keyGet` 提取 key，计算 `crc32(key) % modulo`（日志：`key=SRC hash=N`）。
3. 将计算得到的 hash 写入 `hashCalc.writeBack` 目标。

`RouteByHashRange` 是**终止性**规则：当其返回放行（无 key、无 hashCalc 或 hash 落在间隙）时，规则评估立即停止，后续规则不再评估。

后端服务可一次性计算 hash 并存入 cookie，后续请求网关直接读取：

```python
import zlib
region_hash = zlib.crc32(account_id.encode()) % 1000  # modulo 与配置保持一致
# 写入响应: Set-Cookie: x_region_hash=<region_hash>
```

## Access Log 字段

插件向请求 plugin log 写入条目（128 字节 tier-1 上限，条目均简短）：

| 日志条目 | 含义 |
|---------|------|
| `direct=ohio(from:hdr:X-Direct-Region)` | 从 header 读到 direct override，路由到 ohio |
| `direct unknown 'foo', fallthrough` | 未知 region 名，落入下一规则 |
| `match=hdr:X-Account-Id→virginia` | key 命中 matchMap，路由到 virginia |
| `match=hdr:X-Account-Id no conf` | key 不在 matchMap，落入下一规则 |
| `hash_pass=342 from:cookie:x_region_hash` | 从 cookie 读到预计算 hash |
| `key=hdr:X-Account-Id hash=342` | 从 header 计算得到 hash（不记录原始值） |
| `redirect ohio→virginia` | failoverTo 静态重定向生效 |
| `local, pass` | 目标等于 myRegion，本地放行 |
| `err: failover to invalid` | failoverTo 配置无效（未找到/自指/循环），降级处理 |
| `→virginia 127.0.0.1:30001` | 实际跳转目标（region，host:port） |
| `key missing, pass` | 取不到 key，放行 |

## myRegion 与 failoverTo

### myRegion — 本地放行

当网关实例部署在某个 region 内时，将 `myRegion` 设置为该 region 的名称（在 ClusterRegionRoute PluginMetaData 中配置）。若路由结果等于本 region，插件直接放行，不设 `ExternalJumpPreset`，避免本地流量产生不必要的跨 region 网络跳转。

- 空字符串（默认）= 功能禁用，所有路由均走 `ExternalJumpPreset`。
- 若值不匹配任何 region，运行时静默禁用该功能。

### failoverTo — 静态 Region 重定向

在 `RegionDef` 上配置 `failoverTo`，可将该 region 的流量静态重定向到另一个 region。路由规则确定目标后立即生效。

- `failoverTo` 可在 ClusterRegionRoute PluginMetaData 的 `regions[]` 中配置（影响所有服务）
- 也可在 ServiceRegionRoute PluginMetaData 的 `regions[]` 中配置（只影响该服务），会覆盖集群配置

典型场景：
- **手动 failover**：运维将某 region 标记为下线，流量全量切到备用 region。
- **结合 myRegion 使用**：若重定向目标等于 `myRegion`，重定向后的流量也会本地放行。

约束：
- 目标必须是合法的 region 名称，否则该条重定向静默忽略（access log 记录 `err: failover to invalid`）。
- 只做一层重定向，不链式追踪。
- 不允许自指或循环（运行时静默忽略，记录 `err: failover to invalid`）。

## 注意事项

- `backendEndpoint` 每次请求都会 DNS 解析（复用 Pingora 的 `ExternalJumpPreset` 机制），建议使用内部稳定 DNS 名指向 region 负载均衡器。
- 插件**不修改** Host header 和 SNI。若上游校验 SNI，需确保后端证书覆盖原始 hostname。
- `routeRules: []`（空列表或省略）使插件变为 no-op，可用于分阶段上线。
- ServiceRegionRoute PluginMetaData 必须与 EdgionPlugins 在**同一命名空间**下。ClusterRegionRoute PluginMetaData 可在任意命名空间（通过 `clusterRef` 的 `namespace` 字段指定）。

## 相关文档

- [HTTPRoute 总览](../http-route/overview.md) - HTTPRoute 配置总览
- [负载均衡算法](../http-route/lb-algorithms.md) - 负载均衡策略配置
