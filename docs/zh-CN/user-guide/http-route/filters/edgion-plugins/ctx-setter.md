# Context Setter（CtxSet）插件

> **🔌 Edgion 扩展**
>  
> CtxSet 是 `EdgionPlugins` CRD 提供的上下文变量提取插件，不属于标准 Gateway API。

## 什么是上下文变量？

**上下文变量**是一个插件在请求上计算并存储的命名值，同一请求的后续插件可以读取它。CtxSet 从请求头、Cookie、客户端 IP 等来源提取值，可选地对其进行转换，并以指定名称存储。下游插件（如 RateLimit 和 DSL）则通过该名称消费这个值——例如以租户为维度构建限流 key。

## 快速开始

### 最简配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-plugin
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
```

### 默认值说明

- `vars` 为必填项，且至少包含一条规则。
- 每条规则必须指定恰好一个主值来源：`value`、`template` 或 `from`。若同时设置多个，优先级为 `value` > `template` > `from`。

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `vars` | Array\<Object\> | **必填** | 变量规则列表，不能为空；各规则字段见下方 |

### `vars[]` 规则字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 目标上下文变量名（不能为空） |
| `from` | Object | 无 | 值来源；见下方 `from` 表格。与 `value` / `template` 的优先级互斥 |
| `template` | String | 无 | 带 `${...}` 插值的模板字符串；见模板表格 |
| `value` | String | 无 | 静态字面量值（最高优先级） |
| `extract` | Object | 无 | 对解析后的值应用正则提取 |
| `transform` | Object | 无 | 值转换（大小写、子字符串、前缀、后缀、去除空白、正则替换） |
| `mapping` | Object | 无 | 带可选默认值的值查找表 |
| `default` | String | 无 | 来源为空或提取/转换失败时的回退值 |

### `from` 来源字段

`from` 使用统一的 `KeyGet` 访问器，通过 `type` 字段区分：

| `type` | 额外字段 | 读取来源 |
|--------|----------|----------|
| `remoteIp` | — | 解析后的客户端 IP 地址 |
| `remoteIpAndPath` | — | 解析后的客户端 IP 与请求路径的组合 |
| `header` | `name` | 指定的请求头 |
| `query` | `name` | 指定的查询参数 |
| `cookie` | `name` | 指定的 Cookie |
| `path` | — | 请求路径 |
| `method` | — | HTTP 方法 |
| `ctx` | `name` | 已有的上下文变量 |

如需固定字面量，请在规则上直接使用顶层 `value` 字段而非 `from`。如需组合字符串，请使用 `template`。

### `template` 变量

`template` 是带有 `${...}` 标记的字面量字符串。支持的标记：

| 标记 | 解析为 |
|------|--------|
| `${header:X-Name}` | 请求头的值 |
| `${query:name}` | 查询参数的值 |
| `${cookie:name}` | Cookie 的值 |
| `${ctx:name}` | 已有的上下文变量 |
| `${path}` | 当前规范化路径（不含查询字符串） |
| `${uri}` | 当前规范化完整 URI（路径 + 查询字符串） |
| `${requestPath}` | 入口时的原始路径（冻结，不受后续重写影响） |
| `${requestUri}` | 入口时的原始完整 URI（冻结） |
| `${method}` | HTTP 方法 |
| `${remoteIp}` | 解析后的客户端 IP 地址 |

用户控制的值若自身包含 `${...}`，会作为字面量文本输出，因此模板不存在注入漏洞。

### `extract` 字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `regex` | String | **必填** | 用于匹配值的正则模式 |
| `group` | Integer | `1` | 使用的捕获组（`0` 为整个匹配） |

### `transform` 字段

每条规则最多指定一个转换：

| 参数 | 类型 | 说明 |
|------|------|------|
| `replace` | Object | 正则替换：`{ pattern, with }`。`with` 支持 `$1`、`$2` 捕获引用 |
| `substring` | Array\<Integer\> | `[start, end]`，0 起始，右端不含 |
| `case` | Enum | `upper` 或 `lower` |
| `prefix` | String | 在前面追加固定字符串 |
| `suffix` | String | 在后面追加固定字符串 |
| `trim` | Boolean | 去除两端空白 |

### `mapping` 字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `values` | Object | `{}` | 将输入值映射为输出值的查找表 |
| `default` | String | 无 | 无匹配键时的输出 |

---

## 常见配置场景

### 1. 将请求头复制为上下文变量

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ctx-copy-header
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

### 2. 用模板构建复合 key

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

### 3. 提取、转换并映射套餐等级

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
            transform:
              case: lower
            mapping:
              values:
                premium: "tier_1"
                enterprise: "tier_1"
                basic: "tier_2"
              default: "tier_3"
```

### 4. 串联：CtxSet 为下游 RateLimit 提供 key

CtxSet 在请求阶段先于 RateLimit 运行，因此其设置的变量可被 RateLimit 通过 `ctx` key 来源（`key: { type: ctx, name: <name> }`）读取。两个插件均使用统一的 `KeyGet` 访问器。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-rate-limit
  namespace: production
spec:
  requestPlugins:
    # 1. 从请求中提取租户维度的值
    - type: CtxSet
      config:
        vars:
          - name: "tenant"
            from:
              type: header
              name: "X-Tenant"
            default: "public"
    # 2. RateLimit 读取上面设置的变量
    - type: RateLimit
      config:
        rate: 100
        window: 60
        burst: 20
        key:
          type: ctx
          name: "tenant"
```

---

## 常见问题

### Q1：来源请求头不存在时会怎样？

**A**：若配置了 `default`，变量将被设为该默认值；否则该规则不设置任何变量。下游插件将看到该变量不存在。

### Q2：一条规则能同时使用 `from`、`extract`、`transform` 和 `mapping` 吗？

**A**：可以。流水线按以下顺序执行：解析来源值 → `extract` → `transform` → `mapping`。若结果为空，则应用 `default`。

### Q3：哪些下游插件可以读取这些变量？

**A**：所有在 CtxSet 之后运行的请求阶段插件。RateLimit 通过 key（`{ type: ctx, name: <name> }`）读取，DSL 通过 context API 读取。顺序很重要——请将 CtxSet 放在同一阶段数组中消费者的前面。

---

## 完整配置示例

```yaml
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
            name: tenant-context
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-context
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "tenant"
            from:
              type: header
              name: "X-Tenant"
            transform:
              case: lower
            default: "public"
    - type: RateLimit
      config:
        rate: 100
        window: 60
        burst: 20
        key:
          type: ctx
          name: "tenant"
```

## 相关链接

- [限流插件](./rate-limit.md)
- [DSL 插件](./dsl/README.md)
- [过滤器目录](../README.md)
