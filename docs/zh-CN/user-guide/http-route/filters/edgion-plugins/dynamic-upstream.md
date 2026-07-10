# Dynamic Upstream 插件

> **🔌 Edgion 扩展**
> 
> DynamicInternalUpstream 和 DynamicExternalUpstream 是 `EdgionPlugins` CRD 提供的动态上游路由插件，不属于标准 Gateway API。

## 概述

Dynamic Upstream 允许根据请求中的元数据（Header / Query / Cookie 等）动态选择上游目标。包含两个子类型：

- **DynamicInternalUpstream**：在当前路由已有的 `backendRefs` 中动态选择特定 Service，绕过加权选择。
- **DynamicExternalUpstream**：通过域名映射白名单将流量路由到外部域名。

## DynamicInternalUpstream

### 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: diu-plugin
spec:
  requestPlugins:
    - type: DynamicInternalUpstream
      config:
        from:
          type: header
          name: X-Backend-Target
        onMissing: fallback
```

### 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `from` | Object | **必填** | 路由 key 来源（见子字段） |
| `from.type` | String | `header` | 来源类型：`header` / `query` / `cookie` / `ctx` |
| `from.name` | String | 无 | 来源名称（`header` / `query` / `cookie` / `ctx` 时**必填**） |
| `extract` | Object | 无 | 提取规则，包含 `regex` 和 `group`（默认 group `1`） |
| `rules` | Array\<Object\> | 无 | 匹配规则列表；不设置时使用 direct 模式 |
| `rules[].value` | String | **必填** | 要匹配的路由 key 值（精确匹配） |
| `rules[].backendRef.name` | String | **必填** | 目标 backendRef 名称（必须存在于路由的 `backendRefs` 中） |
| `rules[].backendRef.namespace` | String | 与路由相同 | 目标 backendRef 命名空间；设置时名称和命名空间均需匹配 |
| `onMissing` | Enum | `Fallback` | 路由 key 缺失时的行为。允许值：`Fallback` / `Reject`（400） |
| `onNoMatch` | Enum | `Fallback` | 无规则匹配时的行为（仅 rules 模式）。允许值：`Fallback` / `Reject`（400） |
| `onInvalid` | Enum | `Reject` | 解析出的 backendRef 不在路由中时的行为。允许值：`Reject`（403）/ `Fallback` |
| `dyeHeaders` | Object | 无 | 成功跳转到解析出的后端后应用的流量染色请求头/响应头。`request`/`response` 为 `{name, value}` 条目列表；不设置 `value` 时携带解析出的目标 `[ns/]name`。详见下方[染色请求头](#染色请求头)。 |

### 使用场景

#### Direct 模式

Header 值直接作为目标 Service 名称：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Backend-Target
      onMissing: fallback
      dyeHeaders:
        request:
          - name: X-Dynamic-Internal-Upstream
```

```bash
curl -H "X-Backend-Target: service-v2" https://api.example.com/api
```

#### Rules 模式

通过规则将 Header 值映射到目标 Service：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Version
      rules:
        - value: "v1"
          backendRef:
            name: "api-v1"
        - value: "v2"
          backendRef:
            name: "api-v2"
        - value: "canary"
          backendRef:
            name: "api-canary"
      onNoMatch: fallback
```

#### Direct 模式与 CtxSet 预计算配合

在 direct 模式（无 `rules`）下，路由 key 直接作为目标 backendRef 名称。这与 `CtxSet` 插件配合效果很好——CtxSet 可以从任意请求属性计算目标名称并存储到上下文变量中：

```yaml
requestPlugins:
  # CtxSet 计算目标后端名称并存入上下文
  - type: CtxSet
    config:
      # ... 根据业务逻辑将 ctx 变量 "target_backend" 设为目标名称 ...
  - type: DynamicInternalUpstream
    config:
      from:
        type: ctx
        name: target_backend
      # 无 rules => direct 模式：ctx 值即为 backendRef 名称
      onMissing: fallback
      onInvalid: reject
```

#### 金丝雀 / A-B 测试（基于 cookie）

`DynamicInternalUpstream` 是 Edgion 中其他网关"traffic-split 插件"的等价物：携带指定 cookie 的请求被固定到某个版本，其余请求落回正常的加权后端选择。由于 `from.type` 把 `cookie` 作为一等的具名来源，无需对原始 `Cookie` 头写正则。

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: cookie
        name: canary          # 按名直取 cookie，无需正则匹配 Cookie 头
      rules:
        - value: "v2"
          backendRef:
            name: api-v2
      onMissing: fallback      # 没有 canary cookie => 走正常加权选择（如 90/10）
      onNoMatch: fallback      # cookie 存在但值未知 => 走加权选择
```

```bash
curl -H "Cookie: canary=v2" https://api.example.com/api   # 固定到 api-v2
curl https://api.example.com/api                          # 加权选择
```

> **如何选择金丝雀策略**
> - **确定性、粘性**（同一用户始终看到同一版本）：用本插件配 `cookie` / `header` 来源，即上面的示例。
> - **按百分比逐步放量**（如 5% → 25% → 100%）：用路由的 `backendRefs` `weight`，或插件 `conditions.run` 的 `probability` 匹配器。参见 [后端权重](../../backends/weight.md)。
> - **全量切换、可远程切换**（100% 流量发往同一个后端，无需重新部署即可翻转）：使用带 `activeProfileRef` Selector 的 [Canary 插件](./canary.md)。

---

## DynamicExternalUpstream

### 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: deu-plugin
spec:
  requestPlugins:
    - type: DynamicExternalUpstream
      config:
        from:
          type: header
          name: X-Target-Region
        domainMap:
          "us-west":
            domain: us-west.api.internal
            port: 443
            tls: true
          "eu-central":
            domain: eu-central.api.internal
            port: 443
            tls: true
```

### 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `from` | Object | **必填** | 值来源（见子字段） |
| `extract` | Object | 无 | 提取规则，包含 `regex` 和 `group`（默认 group `1`） |
| `domainMap` | Object | **必填** | 域名映射白名单（不能为空） |
| `onMissing` | Enum | `Skip` | 路由 key 缺失时的行为。允许值：`Skip` / `Reject`（400） |
| `onNoMatch` | Enum | `Skip` | key 无法匹配任何 `domainMap` 条目时的行为。允许值：`Skip` / `Reject`（400） |
| `dyeHeaders` | Object | 无 | 成功跳转到解析出的域名后应用的流量染色请求头/响应头。`request`/`response` 为 `{name, value}` 条目列表；不设置 `value` 时携带解析出的目标 `domain:port`。详见下方[染色请求头](#染色请求头)。 |

### DomainTarget 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `domain` | String | **必填** | 目标域名（主机名；拒绝空格、`/`、`\`、`@`、`#`、`?`、`[`、`]` 及控制字符） |
| `port` | Integer | `tls: true` 时为 `443`，否则为 `80` | 目标端口 |
| `tls` | Boolean | `true` | 是否对上游连接使用 TLS |
| `overrideHost` | String | 原始请求 Host | 覆盖发往外部域名的 Host 头 |
| `sni` | String | `domain` 的值 | 覆盖 TLS SNI（仅在 `tls: true` 时有效） |

### 使用场景

#### 多区域路由

```yaml
requestPlugins:
  - type: DynamicExternalUpstream
    config:
      from:
        type: header
        name: X-Cluster-Target
      extract:
        regex: 'cluster=([\w-]+)'
        group: 1
      domainMap:
        "us-west":
          domain: us-west.api.internal
          port: 443
          tls: true
          overrideHost: api.example.com
        "eu-central":
          domain: eu-central.api.internal
          port: 443
          tls: true
          overrideHost: api.example.com
        "ap-east":
          domain: ap-east.api.internal
          port: 443
          tls: true
      onMissing: skip
      dyeHeaders:
        request:
          - name: X-Dynamic-External-Upstream
```

#### 第三方 API 代理（覆盖 Host + SNI）

将流量路由到外部合作方 API，上游期望在 Host 头中看到其自身主机名，并为不同名称的证书提供服务：

```yaml
requestPlugins:
  - type: DynamicExternalUpstream
    config:
      from:
        type: query
        name: provider
      domainMap:
        "provider-a":
          domain: api-a.example.com
          port: 443
          tls: true
          # 上游按其自身主机名路由
          overrideHost: api-a.example.com
        "provider-b":
          domain: gateway.example.com
          port: 8443
          tls: true
          overrideHost: api-b.example.com
          # 证书 CN 与连接域名不同
          sni: api-b.example.com
      onMissing: reject
      onNoMatch: reject
```

---

## 染色请求头

`dyeHeaders` 默认关闭：不配置时，两个子插件都不会设置任何请求头，也不会剥离任何请求头。配置后，会在成功解析的请求上附加运维自定义的 HTTP 头：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Backend-Target
      dyeHeaders:
        request:
          - name: X-Release-Track      # 不设置 value => 携带解析出的目标 "[ns/]name"
        response:
          - name: X-Served-By
            value: edge-cluster-1      # 显式静态值
```

- `request` 条目设置在转发给上游的请求上；`response` 条目设置在返回给客户端的响应上。
- 不设置 `value` 时携带已解析的选择结果：**`DynamicInternalUpstream`** 使用目标 backendRef 的 `[ns/]name`；**`DynamicExternalUpstream`** 使用目标的 `domain:port`。设置了静态 `value` 则原样发送。
- 只有在成功跳转到解析出的目标后才会染色——`onMissing`/`onNoMatch: skip|fallback` 以及 `onInvalid: reject` 等路径都不会染色。
- **防伪造**：客户端携带的、与配置的**请求方向**染色头同名的请求头，会在插件运行前被剥离——无论最终是跳转、skip/fallback 还是拒绝——因此客户端永远无法伪造后端所信任的标记。
- 请求头名称在准入阶段就会被校验：与消息帧或路由机制冲突的名称（`Host`、`Content-Length`、`Transfer-Encoding`、`Connection`、`TE`、`Upgrade`、`Trailer`）会被拒绝；请求方向染色头名与插件自身 `from` 所读的请求头同名时同样会被拒绝（防伪造剥离会在提取之前把路由键删掉）。
- 协议限定沿用两个插件已有的 gRPC 跳过逻辑：两者都不会给 gRPC 流量染色，因为它们在到达染色调用点之前，就已经在 gRPC 匹配的路由上提前返回。

## 行为细节

- **DynamicInternalUpstream**：选择的目标必须存在于当前路由的 `backendRefs` 中，否则视为无效。
- **DynamicExternalUpstream**：只能路由到 `domainMap` 白名单中的域名，防止 SSRF。
- 在 `Skip` / `Fallback` 模式下，未匹配的请求使用正常路由逻辑。
- `dyeHeaders` 的行为两个子插件共用，详见上方[染色请求头](#染色请求头)。

---

## 常见问题

### Q1：DynamicInternalUpstream 中 `onNoMatch` 和 `onInvalid` 有何区别？

**A**：`onNoMatch` 适用于 rules 模式下路由 key 未匹配任何 `rules` 条目的情况（默认 `Fallback`，回退到正常加权选择）。`onInvalid` 适用于目标已解析但其名称不在路由 `backendRefs` 中的情况（默认 `Reject` 返回 403，因为这通常意味着配置错误）。在 direct 模式下没有 rules，不存在的目标名称会进入 `onInvalid`。

### Q2：为什么 DynamicExternalUpstream 的请求走到了正常后端而不是外部域名？

**A**：默认的 `onMissing` 和 `onNoMatch` 均为 `Skip`，即静默走正常后端选择而非拒绝。若路由 key 缺失或无法匹配 `domainMap` 条目，请求会静默使用路由的 `backendRefs`。若需 fail-close，请将两者均设为 `Reject`，或配置 `dyeHeaders` 确认是否发生了跳转。

### Q3：DynamicExternalUpstream 能访问任意域名吗？

**A**：不能。只有明确列在 `domainMap` 中的域名才能作为目标（白名单模型）。这防止了 SSRF——客户端无法通过路由 key 提供任意域名。

---

## 相关文档

- [DirectEndpoint](./direct-endpoint.md)
- [ProxyRewrite](./proxy-rewrite.md)
- [后端配置](../../backends/README.md)
- [过滤器总览](../overview.md)
