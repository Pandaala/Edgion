# Webhook KeyGet

> **🔌 Edgion 扩展**
>
> WebhookKeyGet 从 LinkSys webhook 解析值并写入请求上下文，每个请求只解析一次。它由 `EdgionPlugins` CRD 提供，不属于标准 Gateway API。

## 什么是 WebhookKeyGet？

WebhookKeyGet 调用 `spec.type: webhook` 的 LinkSys 资源，从响应中提取一个字段，并写入命名的上下文变量。同一请求上的后续插件通过 `from: { type: ctx, name: … }` 读取该变量——它们自己不会再调用 webhook。

需要一个**值**（密钥、租户 ID、补全字段）时用本插件。需要**放行/拒绝裁决**时用 [Forward Auth](./forward-auth.md)。

## 快速开始

### 最简配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-from-webhook
  namespace: production
spec:
  requestPlugins:
    - type: WebhookKeyGet
      config:
        vars:
          - name: tenant_id
            webhookRef: "production/tenant-resolver"
            extract:
              source: body
              path: "data.tenant_id"
            default: "public"
```

被引用的 LinkSys（`production/tenant-resolver`）拥有出站 URL、方法、请求头和超时。WebhookKeyGet 只声明变量名、webhook、提取方式和可选回退值。

### 默认值说明

- `vars` 必填，且至少包含一条。
- 解析在每个请求上**只发生一次**，按列表顺序执行。之后读取同一 ctx 名的插件看到的是已经写入的值。
- 解析失败或为空时，若配置了 `default` 则写入该值；未配置则变量保持未设置。
- 失败不会拒绝请求。未设置或回退值的含义由下游插件决定。

---

## 配置参数

### 基本参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `vars` | Array\<Object\> | **必填** | 要解析的 webhook 变量列表，不能为空 |

### `vars[]` 字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 目标上下文变量名 |
| `webhookRef` | String | **必填** | LinkSys webhook 引用，格式为 `namespace/name` |
| `extract` | Object | **必填** | 如何从 webhook 响应取值；见 extract 表 |
| `pathOverride` | String | 无 | 单次调用路径。仅当 webhook 设置 `request.path.allowOverride: true` 时生效 |
| `default` | String | 无 | 解析无结果时写入的值 |

### `extract` 字段

| `source` | 额外字段 | 读取来源 |
|----------|----------|----------|
| `header` | `name` | 响应头（大小写不敏感） |
| `cookie` | `name` | `Set-Cookie` 名 |
| `body` | `path` | JSON 点路径（`data.user_id`、`result.0.key`） |
| `bodyText` | — | 整个响应体（去首尾空白） |

---

## 常见配置场景

### 1. 从 webhook 响应体解析租户 ID

```yaml
- type: WebhookKeyGet
  config:
    vars:
      - name: tenant_id
        webhookRef: "production/tenant-resolver"
        extract:
          source: body
          path: "data.tenant_id"
        default: "public"
```

### 2. 先 CtxSet，再 WebhookKeyGet

Webhook 模板可以读取 `${ctx:NAME}`。这些名字必须已经存在，因此 CtxSet 必须排在 **WebhookKeyGet 之前**。

```yaml
requestPlugins:
  - type: CtxSet
    config:
      vars:
        - name: action
          value: "resolve"
        - name: user_id
          from:
            type: header
            name: X-User-Id
          default: "anon"
  - type: WebhookKeyGet
    config:
      vars:
        - name: resolved_user
          webhookRef: "production/user-resolver"
          extract:
            source: body
            path: "data.user"
          default: "fallback"
```

单个 CtxSet 条目不要混放本地变量和 webhook 变量。拆开：CtxSet 保留本地变量；WebhookKeyGet 跟在后面。

### 3. WebhookKeyGet 然后 KeyAuth（顺序要求）

KeyAuth 可以认证一个 ctx 变量。WebhookKeyGet 必须**先运行**，变量才会存在。

```yaml
requestPlugins:
  - type: WebhookKeyGet
    config:
      vars:
        - name: api_key
          webhookRef: "production/key-resolver"
          extract:
            source: body
            path: "data.key"
  - type: KeyAuth
    config:
      keySources:
        - type: ctx
          name: api_key
      secretGroups:
        - name: default
          secretRefs:
            - name: api-keys
              namespace: production
```

**把列表写反是静默认证绕过。** 若 KeyAuth 先跑，`api_key` 为空。KeyAuth 会看到没有 key（401）——或者，若它同时还配置了 header/query 来源，就可能用客户端自带的值完成认证，根本不等 webhook。没有跨插件顺序检查（W-1 拒绝过这项检查）。把 WebhookKeyGet 放在每一个读取该变量的插件上面。

---

## 常见问题

### Q1: 为什么上下文变量是空的？

**A**：webhook 调用失败，或 extract 路径未命中，且该变量没有 `default`。检查 LinkSys 的 `Accepted` 条件、extract 路径，以及入站请求是否缺了 `presence: required` 的 origin 字段。

### Q2: 为什么 `pathOverride` 没生效？

**A**：webhook 资源没有设置 `request.path.allowOverride: true`。没有该标志时使用模板路径（或冻结的入站路径）。

### Q3: 什么时候应该改用 Forward Auth？

**A**：外部服务做放行/拒绝裁决时用 Forward Auth。外部服务返回一个供后续插件消费的**值**时用 WebhookKeyGet。

## 获取帮助

如果遇到问题：

1. 查阅本文档的常见问题
2. 查看网关日志
3. 在 GitHub 上开 issue

---

## 安全最佳实践

### ✅ 推荐

- 把 WebhookKeyGet 放在每一个消费该 ctx 变量的插件（KeyAuth、RateLimit、DSL）**之前**。
- 仅当 webhook 才是预期来源时，把 KeyAuth `keySources` 配成 `{ type: ctx, name: … }`——不要顺便再列一个客户端可控的 header，除非这是有意为之。
- `default` 只用于非安全类补全。不要把认证 key 的 default 设成有效凭据。

### ❌ 避免

- 把 KeyAuth（或任何认证插件）放在 WebhookKeyGet 前面，却期望 webhook 值是必需的。
- 把未设置的 ctx 变量当成已认证。
- 在 KeyAuth 或 CtxSet 里再写一遍 webhook 调用。访问器 `type: webhook` 不是面向运维的消费方。

---

## 完整配置示例

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      kind: Service
      name: key-service
      namespace: production
      port: 8080
    request:
      path:
        template: "/resolve"
      method:
        template: "GET"
      headers:
        origin:
          - name: X-Tenant-Id
    timeoutMs: 3000
    success:
      statusCodes: [200]
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: webhook-then-key-auth
  namespace: production
spec:
  requestPlugins:
    - type: WebhookKeyGet
      config:
        vars:
          - name: api_key
            webhookRef: "production/key-resolver"
            extract:
              source: body
              path: "data.key"
    - type: KeyAuth
      config:
        keySources:
          - type: ctx
            name: api_key
        secretGroups:
          - name: default
            secretRefs:
              - name: api-keys
                namespace: production
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected
  namespace: production
spec:
  parentRefs:
    - name: edgion
  hostnames:
    - "api.example.com"
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: webhook-then-key-auth
      backendRefs:
        - name: api-service
          port: 8080
```

---

## 相关链接

- [LinkSys webhook 连接器](../../../../ops-guide/infrastructure/connectors.md)
- [Key Auth](./key-auth.md)
- [Ctx Setter](./ctx-setter.md)
- [Forward Auth](./forward-auth.md)
