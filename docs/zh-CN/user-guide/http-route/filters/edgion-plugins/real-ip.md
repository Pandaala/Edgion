# RealIp 插件

> **🔌 Edgion 扩展**
> 
> RealIp 是 `EdgionPlugins` CRD 提供的客户端真实 IP 提取插件，不属于标准 Gateway API。

## RealIp 解决了什么问题？

当网关位于反向代理、CDN 或云负载均衡器之后时，TCP 连接来自代理而非最终用户。代理会将原始客户端地址记录在 `X-Forwarded-For` 或 `X-Real-IP` 等请求头中。RealIp 插件读取该请求头并解析**真实客户端 IP**，使下游插件和日志看到的是用户地址而非代理地址。

本插件与 nginx 兼容（`ngx_http_realip_module` 语义）：仅当连接对端是已配置的可信代理时才信任该请求头，并可从右到左遍历 `X-Forwarded-For` 链以跳过可信跳点。解析后的 IP 存储在请求上下文中，成为后续插件（例如 [IP Restriction](./ip-restriction.md)）视为客户端 IP 的值。

## 快速开始

### 最简配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: internal
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### 默认值说明

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `trustedIpRefs` | **必填** | 必须非空——无默认信任来源 |
| `realIpHeader` | `X-Forwarded-For` | 读取客户端 IP 的请求头 |
| `recursive` | `true` | 从右到左遍历链，跳过可信 IP |
| `maxTrustedHops` | 无 | 不限制跳过的可信跳点数量 |

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `trustedIpRefs` | Array\<Object\> | **必填** | 非空的可信代理 IpList 引用；所有引用列表必须可用，字段见下表 |
| `realIpHeader` | String | `X-Forwarded-For` | 提取真实 IP 的请求头。必须是合法的 HTTP header 字段名（RFC 9110 `token` 字符集），且不超过 256 字节；非法名称在准入阶段被拒绝。常用值：`X-Forwarded-For`、`X-Real-IP`、`CF-Connecting-IP`、`True-Client-IP` |
| `recursive` | Boolean | `true` | 启用 nginx 风格的从右到左 `X-Forwarded-For` 遍历，跳过可信 IP 直到找到第一个不可信 IP |
| `maxTrustedHops` | Integer | 无 | 限制递归遍历时跳过的可信跳点数量（对应 Envoy 的 `xff_num_trusted_hops` 概念）。设为 `N` 时，跳过 `N` 个可信跳点后停止遍历。仅在 `recursive: true` 时生效。未设置时不限制（与 nginx 兼容） |

### `trustedIpRefs[]`

Kubernetes API admission 若因必填引用、最小数量、目标命名空间或请求头约束拒绝更新，该更新不会到达 Controller/Gateway，上一份已获准的配置保持有效。下文的无效代替换指实际进入类型化发布的配置（例如 FS 输入，或被窄范围保留的混合旧字段），不是 API 已拒绝的更新。

`trustedIpRefs` 按顺序引用 `EdgionConfigData` 的 IpList。IP/CIDR 集合写在 IpList 的 `items`（IpGroup）中；插件只写 `name` 和可选 `namespace`。HTTP 插件默认使用实际所属资源的命名空间；全局 `spec.realIp` 必须显式指定命名空间，并在启用 ReferenceGrant 校验时要求列表当前为 `Cluster` 可见。命名空间级授权不适用于这个集群级引用者。关闭授权校验仍然检查资源身份、IpList 类型和启用状态。

每份引用列表都可用时才信任连接对端和转发链。任意列表缺失、禁用、未授权或类型错误，都在本次解析中使用 PROXY 协议处理后的连接对端，即使另一份列表能匹配。列表内容、可见性、删除和恢复在后续解析中生效，消费者不复制 CIDR；一次快照覆盖对端及全部跳点。中央 IpList 编译保留有效条目并排除无效条目；空列表或全部无效的列表不信任任何地址。

| 字段 | 含义 |
| --- | --- |
| `name` | 必填，IpList 资源名 |
| `namespace` | HTTP 默认所属命名空间；全局 EGC 必填 |

旧的内联 `trustedIps` 及未知字段即使为 null 或空值也会使配置失效。到达类型化发布链路的无效配置会替换上一代：插件封闭请求链，全局配置关闭提取。干净的新一代配置可恢复。全局 EGC 的 `Accepted` 仅反映固有结构，不反映部署本地列表可用性。

### 解析工作原理

- 插件首先检查连接对端是否在 `trustedIpRefs` 中。若**不可信**，直接使用对端地址，忽略请求头（不可信客户端无法通过请求头伪造其 IP）。
- 当对端可信且 `recursive: true`（默认）时，从**右到左**遍历 `X-Forwarded-For` 链，跳过 `trustedIpRefs` 中的所有 IP，返回第一个不可信 IP。
- 递归模式下若链中所有有效 IP 均可信，返回最左侧有效 IP；非递归模式下最右侧有效 IP 可信时使用连接对端。
- `maxTrustedHops` 限制遍历次数：当 `trustedIpRefs` 范围比实际代理链更宽泛时（例如你信任整个 `10.0.0.0/8`，但攻击者也能从该范围访问网关并伪造长链）可作为安全兜底。

---

## 常见配置场景

### 1. 单一可信负载均衡（X-Real-IP，非递归）

当恰好有一个可信 LB 注入 `X-Real-IP` 时，直接读取该值。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: lb
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-xrealip
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Real-IP"
        recursive: false
```

### 2. 多跳代理链（递归 XFF）

信任内部代理网段并向前解析。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: internal
        cidrs:
        - 10.0.0.0/8
        - 192.168.1.0/24
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-recursive
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### 3. CDN 并限制跳点数（Cloudflare）

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: cloudflare
        cidrs:
        - 173.245.48.0/20
        - 103.21.244.0/22
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-cloudflare
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "CF-Connecting-IP"
        recursive: true
        # 最多跳过一个可信跳点
        maxTrustedHops: 1
```

---

## 与 IP Restriction 的配合

RealIp 和 [IP Restriction](./ip-restriction.md) 通常配合使用。RealIp 解析 `RemoteIp` 并存储在请求上下文中；IP Restriction 随后读取该解析地址（`get_remote_ip()`）进行允许/拒绝决策。顺序很重要——两者均在请求阶段运行，因此请将 RealIp 列在 IpRestriction **之前**，使允许/拒绝检查看到的是解析后的客户端身份而非代理 IP：

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: internal
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-then-restrict
  namespace: production
spec:
  requestPlugins:
    # 1. 从可信代理链解析真实客户端 IP
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
    # 2. 基于解析后的客户端 IP 进行允许/拒绝
    - type: IpRestriction
      config:
        allow:
          - name: office
            cidrs:
              - "192.168.1.0/24"
```

---

## 常见问题

### Q1：白名单始终拦截代理后面的真实用户？

**A**：请在 IP Restriction 之前添加 RealIp。没有它，网关看到的是代理地址而非用户地址，白名单永远无法匹配真实客户端。

### Q2：为什么伪造的 `X-Forwarded-For` 仍然被信任？

**A**：不应如此。只有当连接对端在 `trustedIpRefs` 中时，才会信任该请求头；递归遍历会跳过可信跳点以找到第一个不可信 IP。若你信任的范围过宽（例如整个 `10.0.0.0/8`），请缩小 `trustedIpRefs` 范围，或设置 `maxTrustedHops`，防止该范围内的攻击者伪造任意长度的链。

### Q3：网关拒绝了我的配置？

**A**：`trustedIpRefs` 必须非空，`realIpHeader` 必须是有效 HTTP 字段名。IpList 的 CIDR 由中央编译器处理：保留有效条目，排除无效条目；全部引用可用后才进行可信代理解析。

---

## 迁移到 DirectPeerIp 和 RemoteIp

该预发布版本采用一次性硬重命名：`client_addr`/`client-addr` 对应
`DirectPeerIp`/`direct_peer_ip`，`remote_addr`/`remote-addr` 对应
`RemoteIp`/`remote_ip`。请同时更新 `IpSource`、`KeyGet`、模板、DSL、访问日志
查询和后端断言；不保留兼容别名或双写日志字段。DSL 中的
`req.client_ip` → `req.direct_peer_ip` 是需要物理对端的调用方的精确重命名；
普通客户端身份、限流和实验分桶场景应选择 `req.remote_ip`。`RealIp` 继续表示解析机制本身。
向上游传递的 Header 通过
`EdgionGatewayConfig.spec.forwardedHeaders.remoteIpHeader` 独立配置，默认值为
`X-Real-IP`。

## 完整配置示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: internal
        cidrs:
        - 10.0.0.0/8
        - 192.168.1.0/24
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
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
            name: real-ip-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

## 相关链接

- [IP Restriction 插件](./ip-restriction.md)
- [过滤器目录](../README.md)
