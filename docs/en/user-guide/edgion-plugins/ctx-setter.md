# CtxSet Plugin

## Overview

The `CtxSet` plugin sets context variables during request processing. These variables can be used by subsequent plugins (such as RateLimit, PluginConditions) or output as log fields.

CtxSet supports extracting data from multiple sources (Header, Query, Cookie, etc.), and supports default values, case conversion, value mapping, and template-based variable composition.

## Features

- **Multiple data sources**: Extract from Header, Query, Cookie, Path, ClientIP, etc.
- **Value transformation**: Convert extracted values to uppercase or lowercase
- **Value mapping**: Map specific values to other values (e.g., map `plan_id` to `rate_limit_tier`)
- **String templates**: Combine multiple variables using `{{ key }}` syntax
- **Default values**: Use defaults when the data source is missing or mapping fails
- **Static values**: Directly set static strings

## Configuration Parameters

### CtxSetConfig

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `rules` | `[]CtxVarRule` | ✅ | List of setting rules |

### CtxVarRule

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | `string` | ✅ | Target context variable name |
| `from` | `KeyGet` | ❌ | Data source (choose one of `from`, `value`, or `template`) |
| `value` | `string` | ❌ | Static value (as data source) |
| `template` | `string` | ❌ | String template (e.g., `prefix_{{ var }}`) |
| `default` | `string` | ❌ | Default value when data source is missing |
| `transform` | `string` | ❌ | String transformation: `"lower"`, `"upper"` |
| `mapping` | `ValueMapping` | ❌ | Value mapping rules |

### ValueMapping

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `values` | `map[string]string` | ✅ | Mapping table (`original value` -> `new value`) |
| `default` | `string` | ❌ | Default value when mapping doesn't match |

### KeyGet (Data Source)

The `from` field uses the unified `KeyGet` structure:

| Parameter | Type | Description |
|-----------|------|-------------|
| `type` | `string` | Data source type: `header`, `query`, `cookie`, `path`, `method`, `clientIp`, `ctx` |
| `name` | `string` | Key name (required for `header`, `query`, `cookie`, `ctx` types) |

## Configuration Examples

### 1. Basic Usage: Extract from Header

Set the value of the `X-Tenant-Id` header into the `tenant_id` variable. If the header doesn't exist, use `default-tenant`.

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

### 2. Case Conversion

Convert the request method to lowercase and store it in the `method_lower` variable.

```yaml
rules:
  - name: method_lower
    from:
      type: method
    transform: "lower"  # GET -> get
```

### 3. Value Mapping

Map the client-provided `X-Plan` to an internal `tier`.

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
      default: "tier_3"  # Unknown plans default to tier_3
```

**Example logic**:
- `X-Plan: premium` -> `tier: tier_1`
- `X-Plan: basic` -> `tier: tier_3` (default)
- No `X-Plan` -> not set (unless the rule also has a default)

### 4. String Template

Combine ClientIP and Path to generate a unique rate limiting key.

```yaml
rules:
  - name: rate_limit_key
    template: "{{ client_ip }}:{{ path }}"
```

> **Note**: Variables in templates (e.g., `{{ client_ip }}`) must be **existing** context variables or built-in variables. It's recommended to first extract base variables with CtxSet, then combine them with templates.

### 5. Comprehensive Example

Combining multiple features:

```yaml
rules:
  # 1. Extract API version (path cannot directly extract parts, assuming via header or other means)
  - name: api_version
    value: "v1" # Static value example

  # 2. Extract and transform User-Type
  - name: user_type
    from:
      type: header
      name: X-User-Type
    default: "guest"
    transform: "lower"

  # 3. Generate cache key by composition
  - name: cache_key
    template: "cache:{{ api_version }}:{{ user_type }}"
```

## Use Cases

### With RateLimit

The RateLimit plugin can use `ctx` as the data source for the rate limiting key. With CtxSet, you can construct complex rate limiting keys.

```yaml
# 1. Set variables first
- type: CtxSet
  config:
    rules:
      - name: limit_key
        template: "{{ tenant_id }}_{{ remote_addr }}"

# 2. Then apply rate limiting
- type: RateLimit
  config:
    rate: 100
    key:
      source: Ctx
      name: limit_key
```

### With PluginConditions

Conditional plugins can use context variables to decide whether to skip subsequent plugins.

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
