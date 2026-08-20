# CtxSet 插件

> **🔌 Edgion 扩展**
>
> CtxSet 是 `EdgionPlugins` CRD 提供的上下文变量设置插件，不属于标准 Gateway API。

## 概述

`CtxSet` 插件用于在请求处理过程中设置上下文变量。这些变量可以被后续的插件（如 RateLimit、PluginConditions）使用，或者作为日志字段输出。

CtxSet 支持从多种数据源（Header、Query、Cookie、Path、ClientIP、Method、已有上下文变量）提取数据，可选地通过正则提取、转换和值映射对结果进行重塑，并将最终值存储在命名键下供下游插件使用。

## 功能特点

- **多数据源**：支持从 Header、Query、Cookie、Path、Method、ClientIP 或已有上下文变量（`from`）提取
- **静态值**：通过 `value` 直接设置固定字符串
- **模板组合**：使用 `${...}` 标记将多个来源合并为单个字符串
- **正则提取**：通过 `extract` 从解析后的值中提取捕获组
- **值转换**：通过 `transform` 进行大小写转换、子串、前缀/后缀、去空格或正则替换
- **值映射**：通过 `mapping` 将特定值映射为其他值（例如将计划名称映射为限流层级）
- **默认值**：当来源缺失或处理失败时使用兜底值

## 配置参数

### CtxSetConfig

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `vars` | Array\<Object\> | **必填** | 上下文变量规则列表，不能为空；规则字段见下方 |

### `vars[]` 规则字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 目标上下文变量名称（不能为空） |
| `from` | Object | 无 | 值来源；见 `from` 表。优先级：`value` > `template` > `from` |
| `template` | String | 无 | 带 `${...}` 插值的模板字符串；见模板表 |
| `value` | String | 无 | 静态字面量值（最高优先级） |
| `extract` | Object | 无 | 对解析后的值做正则提取 |
| `transform` | Object | 无 | 值转换（大小写、子串、前缀、后缀、去空格、正则替换） |
| `mapping` | Object | 无 | 值查找表，可带默认值 |
| `default` | String | 无 | 来源为空或提取/转换失败时的兜底值 |

### `from` 来源字段

`from` 使用以 `type` 字段标记的统一 `KeyGet` 访问器：

| `type` | 额外字段 | 读取来源 |
|--------|----------|---------|
| `remoteIp` | — | 解析后的客户端 IP 地址 |
| `remoteIpAndPath` | — | 解析后的客户端 IP 与请求路径的组合 |
| `header` | `name` | 指定的请求头 |
| `query` | `name` | 指定的 Query 参数 |
| `cookie` | `name` | 指定的 Cookie |
| `path` | — | 请求路径 |
| `method` | — | HTTP 方法 |
| `ctx` | `name` | 已有的上下文变量，由更早的规则或插件设置 |

若要使用固定字面量，请使用顶层的 `value` 字段而不是 `from`。若要由多个来源构建组合字符串，请使用 `template`。

### `template` 变量

`template` 是带 `${...}` 标记的字面量字符串。支持的标记：

| 标记 | 解析为 |
|------|--------|
| `${header:X-Name}` | 请求头的值 |
| `${query:name}` | Query 参数 |
| `${cookie:name}` | Cookie 值 |
| `${ctx:name}` | 已有的上下文变量 |
| `${path}` | 当前规范化路径（不含 Query 字符串） |
| `${uri}` | 当前规范化完整 URI（路径 + Query） |
| `${requestPath}` | 请求入口时的原始路径（冻结，不受后续重写影响） |
| `${requestUri}` | 请求入口时的原始完整 URI（冻结） |
| `${method}` | HTTP 方法 |
| `${remoteIp}` | 解析后的客户端 IP 地址 |

用户可控的值中如果包含 `${...}` 会被当作字面量文本输出，因此模板不存在注入漏洞。

### `extract` 字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `regex` | String | **必填** | 对值进行匹配的正则表达式 |
| `group` | Integer | `1` | 使用的捕获组（组 `0` 为整个匹配） |

### `transform` 字段

每条规则最多指定一种转换：

| 参数 | 类型 | 说明 |
|------|------|------|
| `replace` | Object | 正则替换：`{ pattern, with }`。`with` 支持 `$1`、`$2` 捕获引用 |
| `substring` | Array\<Integer\> | `[start, end]`，0-indexed，end 不含 |
| `case` | Enum | `upper` 或 `lower` |
| `prefix` | String | 在前面添加固定字符串 |
| `suffix` | String | 在后面追加固定字符串 |
| `trim` | Boolean | 去除两端空格 |

### `mapping` 字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `values` | Object | `{}` | 查找表，将输入值映射为输出值 |
| `default` | String | 无 | 没有键匹配时的输出 |

## 快速开始

最简配置：将请求头复制到上下文变量。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: basic-ctx-setter
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "user_id"
            from:
              type: header
              name: "X-User-Id"
            default: "anonymous"
```

通过 `ExtensionRef` 过滤器将其挂载到 HTTPRoute：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
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
            name: basic-ctx-setter
      backendRefs:
        - name: api-service
          port: 8080
```

## 常见配置场景

### 1. 使用模板构建复合键

将租户头和客户端 IP 合并为单个限流键。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-rate-key
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "rate_key"
            template: "${header:X-Tenant}_${remoteIp}"
```

### 2. 大小写转换

将 HTTP 方法转换为小写供下游使用。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-method
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "method_lower"
            from:
              type: method
            transform:
              case: lower
```

### 3. 值映射（计划 → 层级）

将 `X-Plan` 头映射为内部层级标签，带兜底值。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-tier
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "rate_tier"
            from:
              type: header
              name: "X-Plan"
            mapping:
              values:
                premium: "tier_1"
                enterprise: "tier_1"
                basic: "tier_2"
              default: "tier_3"
```

### 4. 从 JWT Authorization 头中正则提取

从 `Authorization: Bearer <header>.<payload>.<sig>` 中提取 JWT payload 的 subject。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-jwt-user
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "jwt_user"
            from:
              type: header
              name: "Authorization"
            extract:
              regex: "Bearer .*\\.(.+)\\."
              group: 1
            default: "anonymous"
```

### 5. 链式：CtxSet 为下游 RateLimit 提供键

CtxSet 在请求阶段先于 RateLimit 执行，因此它设置的变量可通过 `ctx` 键来源（`key: { type: ctx, name: <name> }`）供 RateLimit 使用。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-rate-limit
  namespace: production
spec:
  requestPlugins:
    # 1. 从请求中提取租户值
    - type: CtxSet
      config:
        vars:
          - name: "tenant"
            from:
              type: header
              name: "X-Tenant"
            default: "public"
    # 2. RateLimit 读取上方设置的变量
    - type: RateLimit
      config:
        rate: 100
        window: 60
        burst: 20
        key:
          type: ctx
          name: "tenant"
```

### 6. 完整流水线：提取、转换、映射

在单条规则中组合提取、转换和映射。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-full-pipeline
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "tenant_id"
            from:
              type: header
              name: "Authorization"
            extract:
              regex: '"tenant":"([^"]+)"'
              group: 1
            transform:
              case: lower
            mapping:
              values:
                acme: "tenant_acme"
                globex: "tenant_globex"
              default: "tenant_default"
            default: "unknown_tenant"
```

## 相关文档

- [CtxSet（HTTPRoute 过滤器视角）](../http-route/filters/edgion-plugins/ctx-setter.md) - 在 HTTPRoute 过滤器中使用 CtxSet
- [RateLimit 插件](rate-limit.md) - 可使用 CtxSet 构造限流键
- [HTTPRoute 总览](../http-route/overview.md) - HTTPRoute 配置总览
