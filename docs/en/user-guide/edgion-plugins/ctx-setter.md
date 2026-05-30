# CtxSet Plugin

> **🔌 Edgion extension**
> 
> CtxSet is a context variable setting plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

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

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rules` | Array\<Object\> | — (required) | List of setting rules, see CtxVarRule sub-fields below |

### CtxVarRule Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | — (required) | Target context variable name |
| `from` | Object | None | Data source (choose one of `from`, `value`, or `template`), see KeyGet below |
| `value` | String | None | Static value (as data source) |
| `template` | String | None | String template (e.g., `prefix_{{ var }}`) |
| `default` | String | None | Default value when data source is missing |
| `transform` | String | None | String transformation. Values: `"lower"` | `"upper"` |
| `mapping` | Object | None | Value mapping rules, see ValueMapping below |

### ValueMapping Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `values` | Object | — (required) | Mapping table (`original value` -> `new value`) |
| `default` | String | None | Default value when mapping doesn't match |

### KeyGet (Data Source) Sub-fields

The `from` field uses the unified `KeyGet` structure:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | String | — (required) | Data source type: `header`, `query`, `cookie`, `path`, `method`, `clientIp`, `ctx`, `webhook` |
| `name` | String | None | Key name (required for `header`, `query`, `cookie`, `ctx` types) |
| `webhookRef` | String | None | LinkSys webhook resource reference `"namespace/name"` (required for `webhook` type) |
| `forwardHeaders` | Array\<String\> | None | List of inbound request header names to forward to the webhook; absent/empty = forward none (optional for `webhook` type) |
| `pathSuffix` | String | None | Path appended to the webhook `uri` (optional for `webhook` type) |
| `extract` | Object | — (required for webhook) | How to extract a value from the webhook response; see below |

#### WebhookExtract Sub-fields

| Parameter | Type | Description |
|-----------|------|-------------|
| `source` | String | Extraction source: `header` (response header), `cookie` (Set-Cookie), `body` (JSON path), `bodyText` (raw text) |
| `name` | String | Required for `header`/`cookie`; the target header / cookie name |
| `path` | String | Required for `body`; dot-path, e.g. `data.user_id`, `items.0.id` |

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

### 5. Webhook Resolution

Resolve a variable value by calling an external service via webhook. First create a LinkSys webhook resource describing the connection:

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

Then reference it from CtxSet:

```yaml
rules:
  # Extract user_id from JSON response body
  - name: user_id
    from:
      type: webhook
      webhookRef: "edgion-default/user-resolver"
      forwardHeaders:                      # forward these inbound headers to the webhook
        - X-Tenant-Id
        - Authorization
      pathSuffix: "/user"                  # actual request: https://api.example.com/v1/resolve/user
      extract:
        source: body
        path: "data.userId"
    default: "anonymous"

  # Extract token from a response header
  - name: api_token
    from:
      type: webhook
      webhookRef: "edgion-default/user-resolver"
      forwardHeaders: [X-Tenant-Id]
      extract:
        source: header
        name: "X-Resolved-Token"

  # Extract from raw response body (plain text)
  - name: body_value
    from:
      type: webhook
      webhookRef: "edgion-default/user-resolver"
      forwardHeaders: [X-Tenant-Id]
      extract:
        source: bodyText
```

**Notes**:
- LinkSys webhook resource connection settings (`auth`, `tls`, `retry`, `rateLimit`, `healthCheck`, `maxResponseBytes`) — see [LinkSys resource documentation](../../dev-guide/resource-architecture-overview.md).
- `forwardHeaders` from the caller is merged with the LinkSys CRD's `defaultHeaders` (if any) — **caller headers win on conflict**.
- Hop-by-hop headers (e.g. `Connection`, `Transfer-Encoding`) are automatically skipped.
- When the webhook call fails, is rate-limited, unhealthy, or returns a non-2xx status, the resolver returns `None` — falling back to `default`.

### 6. Comprehensive Example

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

## Related Documentation

- [ProxyRewrite Plugin](../http-route/filters/edgion-plugins/proxy-rewrite.md) - Request rewrite plugin
- [ResponseRewrite Plugin](../http-route/filters/edgion-plugins/response-rewrite.md) - Response rewrite plugin
- [RateLimit Plugin](rate-limit.md) - Can use CtxSet to construct rate limiting keys
- [HTTPRoute Overview](../http-route/overview.md) - HTTPRoute configuration overview
