# CtxSet 插件

## 概述

`CtxSet` 插件用于在请求处理过程中设置上下文变量（Context Variables）。这些变量可以被后续的插件（如 RateLimiter, PluginConditions）使用，或者作为日志字段输出。

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

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `rules` | `[]CtxVarRule` | ✅ | 设置规则列表 |

### CtxVarRule

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `string` | ✅ | 目标上下文变量名称 |
| `from` | `KeyGet` | ❌ | 数据来源（与 `value` 或 `template` 三选一） |
| `value` | `string` | ❌ | 静态值（作为数据源） |
| `template` | `string` | ❌ | 字符串模板（例如 `prefix_{{ var }}`） |
| `default` | `string` | ❌ | 当数据源缺失时的默认值 |
| `transform` | `string` | ❌ | 字符串转换：`"lower"`, `"upper"` |
| `mapping` | `ValueMapping` | ❌ | 值映射规则 |

### ValueMapping

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `values` | `map[string]string` | ✅ | 映射表 (`原值` -> `新值`) |
| `default` | `string` | ❌ | 映射未匹配时的默认值 |

### KeyGet (数据源)

`from` 字段使用统一的 `KeyGet` 结构：

| 参数 | 类型 | 说明 |
|------|------|------|
| `type` | `string` | 数据源类型：`header`, `query`, `cookie`, `path`, `method`, `clientIp`, `ctx` |
| `name` | `string` | 键名（`header`, `query`, `cookie`, `ctx` 类型必填） |

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

### 5. 综合示例

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

### 配合 RateLimiter 使用

RateLimiter 插件可以使用 `ctx` 作为限流键的数据源。通过 CtxSet，你可以构造复杂的限流键。

```yaml
# 1. 先设置变量
- type: CtxSet
  config:
    rules:
      - name: limit_key
        template: "{{ tenant_id }}_{{ remote_addr }}"

# 2. 后限流
- type: RateLimiter
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
