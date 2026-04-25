# CtxSet 插件

> **🔌 Edgion 扩展**
> 
> CtxSet 是 `EdgionPlugins` CRD 提供的上下文变量设置插件，不属于标准 Gateway API。

## 概述

`CtxSet` 插件用于在请求处理过程中设置上下文变量（Context Variables）。这些变量可以被后续的插件（如 RateLimit, PluginConditions）使用，或者作为日志字段输出。

CtxSet 支持从多种数据源（Header, Query, Cookie 等）提取数据，并支持默认值、大小写转换、值映射（Mapping）以及基于模板的变量组合。

## 功能特性

- **多数据源支持**：支持从 Header, Query, Cookie, Path, ClientIP 等提取数据
- **值转换**：支持将提取的值转换为大写或小写
- **值映射**：支持将特定值映射为其他值（例如将 `plan_id` 映射为 `rate_limit_tier`）
- **字符串模板**：支持使用 `{{ key }}` 语法组合多个变量
- **默认值**：支持在数据源缺失或映射失败时使用默认值
- **静态值**：支持直接设置静态字符串

## 配置参数

### CtxSetConfig

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `rules` | Array\<Object\> | —（必填） | 设置规则列表，见下方 CtxVarRule 子字段 |

### CtxVarRule 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | —（必填） | 目标上下文变量名称 |
| `from` | Object | 无 | 数据来源（与 `value` 或 `template` 三选一），见下方 KeyGet |
| `value` | String | 无 | 静态值（作为数据源） |
| `template` | String | 无 | 字符串模板（例如 `prefix_{{ var }}`） |
| `default` | String | 无 | 当数据源缺失时的默认值 |
| `transform` | String | 无 | 字符串转换。可选值：`"lower"` | `"upper"` |
| `mapping` | Object | 无 | 值映射规则，见下方 ValueMapping |

### ValueMapping 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `values` | Object | —（必填） | 映射表（`原值` -> `新值`） |
| `default` | String | 无 | 映射未匹配时的默认值 |

### KeyGet（数据源）子字段

`from` 字段使用统一的 `KeyGet` 结构：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | String | —（必填） | 数据源类型：`header`、`query`、`cookie`、`path`、`method`、`clientIp`、`ctx`、`webhook` |
| `name` | String | 无 | 键名（`header`、`query`、`cookie`、`ctx` 类型必填） |
| `webhookRef` | String | 无 | 引用的 LinkSys webhook 资源 `"namespace/name"`（`webhook` 类型必填） |
| `forwardHeaders` | Array\<String\> | 无 | 从入站请求转发到 webhook 的 header 名单；缺省/空 = 不额外转发（`webhook` 类型可选） |
| `pathSuffix` | String | 无 | 拼接到 webhook `uri` 后的 path（`webhook` 类型可选） |
| `extract` | Object | —（webhook 必填） | 从 webhook 响应中提取值的方式，见下方 |

#### WebhookExtract 子字段

| 参数 | 类型 | 说明 |
|------|------|------|
| `source` | String | 提取来源：`header`（响应 header）、`cookie`（Set-Cookie）、`body`（JSON 路径）、`bodyText`（原始文本） |
| `name` | String | `header`/`cookie` 类型必填，目标 header / cookie 名 |
| `path` | String | `body` 类型必填，dot 路径，例如 `data.user_id`、`items.0.id` |

## 配置示例

### 1. 基础用法：从 Header 提取

将 `X-Tenant-Id` Header 的值设置到 `tenant_id` 变量中。如果 Header 不存在，使用 `default-tenant`。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: basic-ctx-setter
spec:
  requestPlugins:
    - type: CtxSet
      config:
        rules:
          - name: tenant_id
            from:
              type: header
              name: X-Tenant-Id
            default: "default-tenant"
```

### 2. 大小写转换

将请求方法转换为小写，存储到 `method_lower` 变量中。

```yaml
rules:
  - name: method_lower
    from:
      type: method
    transform: "lower"  # GET -> get
```

### 3. 值映射 (Mapping)

将客户端传递的 `X-Plan` 映射为内部使用的 `tier`。

```yaml
rules:
  - name: tier
    from:
      type: header
      name: X-Plan
    mapping:
      values:
        "premium": "tier_1"
        "standard": "tier_2"
      default: "tier_3"  # 未知 plan 默认为 tier_3
```

**示例逻辑**：
- `X-Plan: premium` -> `tier: tier_1`
- `X-Plan: basic` -> `tier: tier_3` (默认值)
- 无 `X-Plan` -> 未设置 (除非规则级也有 default)

### 4. 字符串模板

组合 ClientIP 和 Path 生成一个唯一的限流 Key。

```yaml
rules:
  - name: rate_limit_key
    template: "{{ client_ip }}:{{ path }}"
```

> **注意**：模板中的变量（如 `{{ client_ip }}`）必须是 **已存在** 的上下文变量或内置变量。建议先用 CtxSet 提取基础变量，再用模板组合。

### 5. Webhook 调用解析

通过 webhook 调用外部服务解析变量值。需要先创建 LinkSys webhook 资源描述目标连接：

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: user-resolver
  namespace: edgion-default
spec:
  type: webhook
  config:
    uri: "https://api.example.com/v1/resolve"
    requestMethod: GET
    timeoutMs: 3000
    successStatusCodes: [200]
```

然后在 CtxSet 里引用：

```yaml
rules:
  # 从 webhook 响应 JSON 提取 user_id
  - name: user_id
    from:
      type: webhook
      webhookRef: "edgion-default/user-resolver"
      forwardHeaders:                      # 把入站请求的这些 header 透传给 webhook
        - X-Tenant-Id
        - Authorization
      pathSuffix: "/user"                  # 实际请求 https://api.example.com/v1/resolve/user
      extract:
        source: body
        path: "data.userId"
    default: "anonymous"

  # 从 webhook 响应 header 提取 token
  - name: api_token
    from:
      type: webhook
      webhookRef: "edgion-default/user-resolver"
      forwardHeaders: [X-Tenant-Id]
      extract:
        source: header
        name: "X-Resolved-Token"

  # 从 webhook 响应纯文本提取
  - name: body_value
    from:
      type: webhook
      webhookRef: "edgion-default/user-resolver"
      forwardHeaders: [X-Tenant-Id]
      extract:
        source: bodyText
```

**注意**：
- LinkSys webhook 资源的 `auth`、`tls`、`retry`、`rateLimit`、`healthCheck`、`maxResponseBytes` 等连接级配置见 [LinkSys 资源文档](../../dev-guide/resource-architecture-overview.md)。
- 调用方传入的 `forwardHeaders` 与 LinkSys CRD `defaultHeaders`（如有）合并发往 webhook，**调用方传入的 header 优先级更高**。
- Hop-by-hop header（如 `Connection`、`Transfer-Encoding`）会自动跳过转发。
- Webhook 调用失败、限流、不健康、响应非 2xx 时返回 `None`，触发 `default` 兜底。

### 6. 综合示例

结合多种功能：

```yaml
rules:
  # 1. 提取 API 版本 (path 无法直接提取部分，假设由其他方式或通过 header)
  - name: api_version
    value: "v1" # 静态值示例

  # 2. 提取并转换 User-Type
  - name: user_type
    from:
      type: header
      name: X-User-Type
    default: "guest"
    transform: "lower"

  # 3. 组合生成缓存 Key
  - name: cache_key
    template: "cache:{{ api_version }}:{{ user_type }}"
```

## 使用场景

### 配合 RateLimit 使用

RateLimit 插件可以使用 `ctx` 作为限流键的数据源。通过 CtxSet，你可以构造复杂的限流键。

```yaml
# 1. 先设置变量
- type: CtxSet
  config:
    rules:
      - name: limit_key
        template: "{{ tenant_id }}_{{ remote_addr }}"

# 2. 后限流
- type: RateLimit
  config:
    rate: 100
    key:
      source: Ctx
      name: limit_key
```

### 配合 PluginConditions 使用

条件判断插件可以基于上下文变量决定是否跳过后续插件。

```yaml
- type: CtxSet
  config:
    rules:
      - name: is_internal
        from:
          type: clientIp
        mapping:
          values:
             "127.0.0.1": "true"
          default: "false"
```

## 相关文档

- [ProxyRewrite 插件](../http-route/filters/edgion-plugins/proxy-rewrite.md) - 请求改写插件
- [ResponseRewrite 插件](../http-route/filters/edgion-plugins/response-rewrite.md) - 响应改写插件
- [RateLimit 插件](rate-limit.md) - 可使用 CtxSet 构造限流键
- [HTTPRoute 总览](../http-route/overview.md) - HTTPRoute 配置总览
