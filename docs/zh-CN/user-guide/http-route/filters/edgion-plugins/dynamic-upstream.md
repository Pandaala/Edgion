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
| `rules[].backendRef.name` | String | **必填** | 目标 backendRef 名称（必须存在于路由的 `backendRefs` 中，且未被 ReferenceGrant 拒绝） |
| `rules[].backendRef.namespace` | String | 与路由相同 | 目标 backendRef 命名空间；设置时名称和命名空间均需匹配 |
| `onMissing` | Enum | `Fallback` | 路由 key 缺失时的行为。允许值：`Fallback` / `Reject`（400） |
| `onNoMatch` | Enum | `Fallback` | 无规则匹配时的行为（仅 rules 模式）。允许值：`Fallback` / `Reject`（400） |
| `onInvalid` | Enum | `Reject` | 解析出的 backendRef 不可用时的行为——不在路由中，或虽在路由中但被 ReferenceGrant 拒绝。允许值：`Reject`（403）/ `Fallback` |

`dye` 是**条目级（entry-level）**字段——与 `type`/`config`/`enable` 同级，不属于 `config` 的成员：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `dye` | Object | 无 | 按结果分类的流量染色请求头/响应头（`on: [success, fallback, reject]`）。`request`/`response` 为 `{name, on, value, conditions}` 条目列表；不设置 `value` 时，`success` 上携带解析出的目标 `[ns/]name`。完整参考见 [染色（Dye）](./dye.md)，插件特有内容见下方[染色请求头](#染色请求头)。 |

### 使用场景

#### Direct 模式

Header 值直接作为目标 Service 名称：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    dye:
      request:
        - name: X-Dynamic-Internal-Upstream
    config:
      from:
        type: header
        name: X-Backend-Target
      onMissing: fallback
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
| `onMissing` | Enum | `Fallback` | 路由 key 缺失时的行为。允许值：`Fallback` / `Reject`（400） |
| `onNoMatch` | Enum | `Fallback` | key 无法匹配任何 `domainMap` 条目时的行为。允许值：`Fallback` / `Reject`（400） |

`dye` 是**条目级（entry-level）**字段——与 `type`/`config`/`enable` 同级，不属于 `config` 的成员：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `dye` | Object | 无 | 按结果分类的流量染色请求头/响应头（`on: [success, fallback, reject]`）。`request`/`response` 为 `{name, on, value, conditions}` 条目列表；不设置 `value` 时，`success` 上携带解析出的目标 `domain:port`。完整参考见 [染色（Dye）](./dye.md)，插件特有内容见下方[染色请求头](#染色请求头)。 |

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
    dye:
      request:
        - name: X-Dynamic-External-Upstream
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
      onMissing: fallback
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

完整参考见 [染色（Dye）](./dye.md)：`on` 结果词表、两种 `value` 来源、模板引擎、防伪造剥离（即使插件被 `enable: false` 关闭或被 conditions 跳过也会执行）、请求头名校验，以及安全指引。本节只覆盖这两个插件特有的部分。

`dye` 默认关闭：不配置时，两个子插件都不会设置任何请求头，也不会剥离任何请求头。配置后，会附加运维自定义的 HTTP 头：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    dye:
      request:
        - name: X-Release-Track      # 不设置 value => 已解析的目标 "[ns/]name"（默认 on: [success]）
      response:
        - name: X-Served-By
          value: edge-cluster-1      # 显式静态值
        - name: X-Edgion-Outcome
          on: [fallback, reject]     # 不设置 value => dynamic_internal_upstream:fallback:on-missing 等
    config:
      from:
        type: header
        name: X-Backend-Target
```

- `success` 上不设置 `value` 时携带已解析的选择结果：**`DynamicInternalUpstream`** 使用目标 backendRef 的 `[ns/]name`；**`DynamicExternalUpstream`** 使用目标的 `domain:port`。
- 两个插件都支持 `success`、`fallback`、`reject` 三种染色能力——`fallback`/`reject` 规则还会给 `onMissing`/`onNoMatch` 分支染色（取决于配置的是 `fallback` 还是 `reject`），以及（仅 `DynamicInternalUpstream`）`onInvalid` 分支（`DynamicExternalUpstream` 没有 `onInvalid`，因为 `domainMap` 的归属校验本身就内含在匹配过程中）。默认 `on`（即 `success`）不会在这些路径上触发。
- 协议限定沿用两个插件已有的 gRPC 跳过逻辑：两者都不会给 gRPC 流量*染色*，因为它们在到达记录结果的调用点之前，就已经在 gRPC 匹配的路由上提前返回。但**剥离**依然会作用于 gRPC 流量——剥离是运行时统一负责、无条件执行的（见[染色（Dye）](./dye.md)），因此客户端无法借助 gRPC 请求，在同时承载 HTTP 流量的路由上偷渡伪造的染色头。

## 行为细节

- **DynamicInternalUpstream**：选择的目标必须存在于当前路由的 `backendRefs` 中且可用——Controller 因缺少匹配的 `ReferenceGrant` 而拒绝的跨命名空间引用，与不存在的目标一样视为无效，走 `onInvalid`。
- **DynamicExternalUpstream**：只能路由到 `domainMap` 白名单中的域名，防止 SSRF。
- 在 `Fallback` 模式下，未匹配的请求使用正常路由逻辑。
- `dye` 的行为两个子插件共用，详见上方[染色请求头](#染色请求头)。

---

## 常见问题

### Q1：DynamicInternalUpstream 中 `onNoMatch` 和 `onInvalid` 有何区别？

**A**：`onNoMatch` 适用于 rules 模式下路由 key 未匹配任何 `rules` 条目的情况（默认 `Fallback`，回退到正常加权选择）。`onInvalid` 适用于目标已解析但路由无法使用它的情况——其名称不在路由 `backendRefs` 中，或虽然存在但属于 Controller 因缺少匹配的 `ReferenceGrant` 而拒绝的跨命名空间引用（默认 `Reject` 返回 403，因为这两种情况通常都意味着配置错误）。在 direct 模式下没有 rules，不存在的目标名称会进入 `onInvalid`。

### Q2：为什么 DynamicExternalUpstream 的请求走到了正常后端而不是外部域名？

**A**：默认的 `onMissing` 和 `onNoMatch` 均为 `Fallback`，即静默走正常后端选择而非拒绝。若路由 key 缺失或无法匹配 `domainMap` 条目，请求会静默使用路由的 `backendRefs`。若需 fail-close，请将两者均设为 `Reject`，或配置 `dye` 确认是否发生了跳转。

### Q3：DynamicExternalUpstream 能访问任意域名吗？

**A**：不能。只有明确列在 `domainMap` 中的域名才能作为目标（白名单模型）。这防止了 SSRF——客户端无法通过路由 key 提供任意域名。

---

## 相关文档

- [DirectEndpoint](./direct-endpoint.md)
- [ProxyRewrite](./proxy-rewrite.md)
- [后端配置](../../backends/README.md)
- [过滤器总览](../overview.md)
