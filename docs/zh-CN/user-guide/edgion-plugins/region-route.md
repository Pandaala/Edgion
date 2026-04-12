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
- `baseInfo.myRegion`：若路由结果等于本网关所在 region，插件直接放行，不设 `ExternalJumpPreset`（流量已在本地，无需跨 region 跳转）。
- `failoverTo`：per-region 静态重定向——该 region 的流量在健康检查前先重定向到另一个 region。
- 可选的 auto failover 通过 DNS 探测每个 region 的健康状态；若某 region DNS 无返回，流量自动切到最健康的其他 region。
- `routeRules` 为空 → 插件 no-op。

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `baseInfo` | Object | —（必填） | 基础信息包装对象，包含 region 定义及 Center 联邦聚合所需的元数据 |
| `baseInfo.serviceGroup` | String | —（必填，DNS-1123 label） | 服务组名称，用于分组相关的 RegionRoute 插件。Center 联邦模式下，同一 `serviceGroup` 的插件会被聚合到一起进行全局视图展示和统一管理 |
| `baseInfo.cluster` | String | `""` | 集群标识符（如 `"us-prod"`、`"eu-staging"`），用于在 Center 联邦模式下区分来自不同集群的 RegionRoute 配置 |
| `baseInfo.regions` | Array\<Object\> | —（必填） | Region 定义列表，见下方 `RegionDef` 子字段 |
| `baseInfo.myRegion` | String | `""` | 本网关所在 region 名称，路由目标等于此值时直接放行 |
| `keyGet` | Array\<Object\> | `[]` | 共享的 key 提取配置（RouteByKeyConfMatch 和 RouteByHashRange 共用） |
| `hashKeyGet` | Array\<Object\> | 无 | 预计算 hash 读取源（RouteByHashRange 计算前先尝试读取） |
| `hashCalc` | Object | 无 | Hash 计算配置，缺省时 RouteByHashRange 仅做 passthrough |
| `routeRules` | Array\<Object\> | `[]` | 有序路由规则列表，空列表时插件为 no-op |
| `autoFailover` | Object | 无 | 自动 failover 配置（基于 DNS 探测） |

### `regions[]`（RegionDef）子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | —（必填） | Region 标识名称 |
| `hashRange` | Array\<Integer\> | —（必填） | Hash 范围 `[low, high]`，闭区间，须满足 `high < modulo` |
| `backendEndpoint` | String | —（必填） | 后端端点，格式 `host` 或 `host:port` |
| `tls` | Boolean | `true` | 上游连接是否使用 TLS |
| `failoverTo` | String | 无 | 静态 failover 目标 region 名称 |

### `hashCalc` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `algorithm` | Enum | `crc32` | Hash 算法。可选值：`crc32` | `sha256` |
| `modulo` | Integer | `1000` | Hash 值域 `[0, modulo-1]`，须 >= 2 |
| `writeBack` | Array\<Object\> | `[]` | 将计算得到的 hash 写入目标列表 |

### `autoFailover` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | `true` | 是否启用自动 failover |
| `healthyThreshold` | Float | `0.5` | 健康阈值，DNS 无返回时触发 failover |
| `probeIntervalMs` | Integer | `5000` | DNS 探测间隔（毫秒） |
| `notifyHeader` | String | 无 | 写入响应 header 的 failover region 名称（便于调试） |
| `onAllRegionsDown` | Enum | `pass` | 所有 region 不可用时的行为。可选值：`pass`（放行） | `reject`（返回 503） |

---

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route
  namespace: default
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        baseInfo:
          # 服务组名称 — Center 联邦模式下用于聚合同一服务的 RegionRoute 配置
          serviceGroup: my-service
          # 可选：集群标识符 — 区分不同集群的配置（如 "us-prod"、"eu-staging"）
          # cluster: us-prod

          # region 定义 — hashRange [low, high]，闭区间
          regions:
            - name: virginia
              hashRange: [0, 599]           # 60% 流量（1000 槽中的 600 个）
              backendEndpoint: virginia-internal.example.com:8443
              tls: true
              # failoverTo: ohio            # 可选：将 virginia 流量静态重定向到 ohio
            - name: ohio
              hashRange: [600, 999]         # 40% 流量（1000 槽中的 400 个）
              backendEndpoint: ohio-internal.example.com:8443
              tls: true
              # failoverTo: virginia        # 可选：将 ohio 流量静态重定向到 virginia

          # 可选：本网关所在 region——路由目标等于此值时直接放行（本地流量，无跨 region 跳转）
          # myRegion: virginia

        # RouteByKeyConfMatch 和 RouteByHashRange 共用的 key 提取配置
        keyGet:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id

        # RouteByHashRange 的预计算 hash 读取源（计算前先尝试读取）
        hashKeyGet:
          - type: cookie
            name: x_region_hash

        # hash 计算配置 — 若缺省，RouteByHashRange 仅做 passthrough
        hashCalc:
          algorithm: crc32     # crc32（默认）| sha256
          modulo: 1000         # 1000 个槽 [0, 999]；hashRange[1] 必须 < modulo
          writeBack:           # 将计算得到的 hash 写入以下目标
            - type: responseHeader
              name: X-Region-Hash

        # 有序路由规则 — 按顺序评估，第一个匹配生效
        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Direct-Region

          - type: RouteByKeyConfMatch
            matchMap:
              "vip-001": virginia
              "enterprise-xyz": ohio

          - type: RouteByHashRange   # 终止性规则，无单独配置

        # 可选：auto failover（基于 DNS 探测）
        autoFailover:
          enabled: true
          healthyThreshold: 0.5          # DNS 无返回时触发
          probeIntervalMs: 5000          # 每 5s 探测一次
          notifyHeader: X-Failover-Region    # 写入响应 header（便于 debug）
          onAllRegionsDown: pass         # pass | reject（返回 503）
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

插件向请求 plugin log 写入条目（100 字节上限，条目均简短）：

| 日志条目 | 含义 |
|---------|------|
| `direct=ohio(from:hdr:X-Direct-Region);` | 从 header 读到 direct override，路由到 ohio |
| `direct unknown 'foo', fallthrough;` | 未知 region 名，落入下一规则 |
| `match=hdr:X-Account-Id→virginia;` | key 命中 matchMap，路由到 virginia |
| `match=hdr:X-Account-Id no conf;` | key 不在 matchMap，落入下一规则 |
| `hash_pass=342 from:cookie:x_region_hash;` | 从 cookie 读到预计算 hash |
| `key=hdr:X-Account-Id hash=342;` | 从 header 计算得到 hash（不记录原始值） |
| `redirect ohio→virginia;` | failoverTo 静态重定向生效 |
| `local, pass;` | 目标等于 myRegion，本地放行 |
| `err:failoverTo invalid;` | failoverTo 配置无效（未找到/自指/循环），降级处理 |
| `→virginia 127.0.0.1:30001;` | 实际跳转目标（region，host:port） |
| `failover→ohio (100%);` | autoFailover 自动切换到 ohio |
| `key missing, pass;` | 取不到 key，放行 |

## myRegion 与 failoverTo

### myRegion — 本地放行

当网关实例部署在某个 region 内时，将 `baseInfo.myRegion` 设置为该 region 的名称。若路由结果等于本 region，插件直接放行，不设 `ExternalJumpPreset`，避免本地流量产生不必要的跨 region 网络跳转。

```yaml
baseInfo:
  serviceGroup: my-service
  myRegion: virginia    # 本网关部署在 virginia
```

- 空字符串（默认）= 功能禁用，所有路由均走 `ExternalJumpPreset`。
- 若值不匹配任何 region，运行时静默禁用该功能。

### failoverTo — 静态 Region 重定向

在任意 `RegionDef` 上配置 `failoverTo`，可将该 region 的流量静态重定向到另一个 region。在路由规则确定目标之后、`autoFailover` 健康检查之前生效。

```yaml
baseInfo:
  serviceGroup: my-service
  regions:
    - name: ohio
      hashRange: [600, 999]
      backendEndpoint: ohio-internal.example.com:8443
      failoverTo: virginia     # ohio 的所有流量 → 改走 virginia
```

典型场景：
- **手动 failover**：运维将某 region 标记为下线，流量全量切到备用 region。
- **结合 myRegion 使用**：若重定向目标等于 `baseInfo.myRegion`，重定向后的流量也会本地放行。

约束：
- 目标必须是合法的 region 名称，否则该条重定向静默忽略（access log 记录 `err:failoverTo invalid;`）。
- 只做一层重定向，不链式追踪。
- 不允许自指或循环（运行时静默忽略，记录 `err:failoverTo invalid;`）。

## 注意事项

- `backendEndpoint` 每次请求都会 DNS 解析（复用 Pingora 的 `ExternalJumpPreset` 机制），建议使用内部稳定 DNS 名指向 region 负载均衡器。
- 插件**不修改** Host header 和 SNI。若上游校验 SNI，需确保后端证书覆盖原始 hostname。
- Auto failover 仅基于 DNS 活性探测（DNS 是否返回 IP），不做应用层健康检查。
- `routeRules: []`（空列表或省略）使插件变为 no-op，可用于分阶段上线。

## 相关文档

- [HTTPRoute 总览](../http-route/overview.md) - HTTPRoute 配置总览
- [负载均衡算法](../http-route/lb-algorithms.md) - 负载均衡策略配置
