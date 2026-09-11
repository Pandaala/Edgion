# CtxSet Plugin

> **🔌 Edgion extension**
>
> CtxSet is a context variable setting plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The `CtxSet` plugin sets context variables during request processing. These variables can be used by subsequent plugins (such as RateLimitLocal, PluginConditions). Access-log output requires an explicit `from: ctx` entry in `EdgionPlugins.spec.accessLogExtern`; values are masked by default and sensitive context keys are categorically blocked. See [AccessLogExtern](../../ops-guide/observability/access-log.md).

CtxSet supports extracting data from multiple sources (Header, Query, Cookie, Path, ClientIP, Method, existing context vars), optionally reshapes the value with regex extraction, transformation, and value mapping, and stores the result under a named key for downstream plugins.

## Features

- **Multiple data sources**: Extract from Header, Query, Cookie, Path, Method, ClientIP, or an existing context variable (`from`)
- **Static values**: Directly set a fixed string with `value`
- **Template composition**: Combine multiple sources into a single string with `${...}` markers
- **Regex extraction**: Pull a capture group from the resolved value with `extract`
- **Value transformation**: Case conversion, substring, prefix/suffix, trim, or regex replace via `transform`
- **Value mapping**: Map specific values to other values (e.g., plan name → rate-limit tier) via `mapping`
- **Default values**: Use a fallback when the source is missing or processing fails

## Configuration Parameters

### CtxSetConfig

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `vars` | Array\<Object\> | **Required** | List of context variable rules. Must be non-empty; see rule fields below |

### `vars[]` rule fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Target context variable name (must be non-empty) |
| `from` | Object | none | Value source; see the `from` table. Priority: `value` > `template` > `from` |
| `template` | String | none | Template string with `${...}` interpolation; see the template table |
| `value` | String | none | Static literal value (highest priority) |
| `extract` | Object | none | Regex extraction applied to the resolved value |
| `transform` | Object | none | Value transformation (case, substring, prefix, suffix, trim, regex replace) |
| `mapping` | Object | none | Value lookup table with optional default |
| `default` | String | none | Fallback when the source is empty or extraction/transform fails |

### `from` source fields

`from` uses the unified `KeyGet` accessor, tagged by a `type` field:

| `type` | Extra field | Reads from |
|--------|-------------|------------|
| `remoteIp` | — | The resolved client IP address |
| `remoteIpAndPath` | — | Resolved client IP combined with the request path |
| `header` | `name` | The named request header |
| `query` | `name` | The named query-string parameter |
| `cookie` | `name` | The named cookie |
| `path` | — | The request path |
| `method` | — | The HTTP method |
| `ctx` | `name` | An existing context variable set by an earlier rule or plugin |

For a fixed literal, use the top-level `value` field instead of `from`. For a composite string built from multiple sources, use `template`. To resolve a value from a LinkSys webhook, use [WebhookKeyGet](../http-route/filters/edgion-plugins/webhook-key-get.md) — not a CtxSet `from` source.

### `template` variables

A `template` is a literal string with `${...}` markers. Supported markers:

| Marker | Resolves to |
|--------|-------------|
| `${header:X-Name}` | A request header value |
| `${query:name}` | A query-string parameter |
| `${cookie:name}` | A cookie value |
| `${ctx:name}` | An existing context variable |
| `${path}` | Current normalized path (no query string) |
| `${uri}` | Current normalized full URI (path + query) |
| `${requestPath}` | Entry-time raw path (frozen, immune to later rewrites) |
| `${requestUri}` | Entry-time raw full URI (frozen) |
| `${method}` | HTTP method |
| `${remoteIp}` | Resolved client IP address |

User-controlled values that contain `${...}` are emitted as literal text, so templates are not vulnerable to injection.

### `extract` fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `regex` | String | **Required** | Regex pattern to match against the value |
| `group` | Integer | `1` | Capture group to use (group `0` is the whole match) |

### `transform` fields

Specify at most one transformation per rule:

| Parameter | Type | Description |
|-----------|------|-------------|
| `replace` | Object | Regex replace: `{ pattern, with }`. `with` supports `$1`, `$2` capture references |
| `substring` | Array\<Integer\> | `[start, end]`, 0-indexed, exclusive end |
| `case` | Enum | `upper` or `lower` |
| `prefix` | String | Prepend a fixed string |
| `suffix` | String | Append a fixed string |
| `trim` | Boolean | Trim whitespace from both ends |

### `mapping` fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `values` | Object | `{}` | Lookup table mapping input value → output value |
| `default` | String | none | Output when no key matches |

## Quick Start

The simplest configuration: copy a request header into a context variable.

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

Wire it to an HTTPRoute via an `ExtensionRef` filter:

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

## Common Configuration Scenarios

### 1. Build a composite key with a template

Combine the tenant header and the client IP into a single rate-limit key.

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

### 2. Case conversion

Convert the HTTP method to lowercase for downstream use.

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

### 3. Value mapping (plan → tier)

Map the `X-Plan` header to an internal tier label, with a fallback.

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

### 4. Regex extraction from a JWT Authorization header

Extract the JWT payload subject from `Authorization: Bearer <header>.<payload>.<sig>`.

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

### 5. Chain: CtxSet feeds a downstream RateLimitLocal key

CtxSet runs in the request phase before RateLimitLocal, so the variable it sets is available to RateLimitLocal through a `ctx` key source (`key: { type: ctx, name: <name> }`).

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-rate-limit
  namespace: production
spec:
  requestPlugins:
    # 1. Derive a per-tenant value from the request
    - type: CtxSet
      config:
        vars:
          - name: "tenant"
            from:
              type: header
              name: "X-Tenant"
            default: "public"
    # 2. RateLimitLocal reads the variable set above
    - type: RateLimitLocal
      config:
        rate: 100
        window: 60
        burst: 20
        key:
          type: ctx
          name: "tenant"
```

### 6. Full pipeline: extract, transform, and map

Combine extraction, transformation, and mapping in a single rule.

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

## Related Documentation

- [CtxSet (HTTPRoute filter view)](../http-route/filters/edgion-plugins/ctx-setter.md) - Using CtxSet within an HTTPRoute filter
- [Webhook KeyGet](../http-route/filters/edgion-plugins/webhook-key-get.md) - Resolve a LinkSys webhook value into context
- [RateLimitLocal Plugin](rate-limit-local.md) - Can use CtxSet to construct rate limiting keys
- [HTTPRoute Overview](../http-route/overview.md) - HTTPRoute configuration overview
