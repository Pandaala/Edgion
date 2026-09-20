# Context Setter (CtxSet) Plugin

> **🔌 Edgion extension**
> 
> CtxSet is a context-variable plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What is a context variable?

A **context variable** is a named value that one plugin computes and stores on the request, so that later plugins on the same request can read it. CtxSet extracts a value (from a header, cookie, the client IP, etc.), optionally reshapes it, and stores it under a name. Downstream plugins such as RateLimitLocal and DSL then consume that name — for example, to build a rate-limit key per tenant.

## Quick Start

### Simplest configuration

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

### Default-value notes

- `vars` is required and must contain at least one rule.
- Each rule must specify exactly one primary value source: `value`, `template`, or `from`. If more than one is set, the priority is `value` > `template` > `from`.

---

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `vars` | Array\<Object\> | **Required** | List of variable rules. Must be non-empty; see the rule fields below |

### `vars[]` rule fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Target context variable name (must be non-empty) |
| `from` | Object | none | Value source; see the `from` table. Mutually exclusive in priority with `value` / `template` |
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
| `ctx` | `name` | An existing context variable |

For a fixed literal, use the top-level `value` field on the rule instead of `from`. For a composite string, use `template`. To resolve a value from a LinkSys webhook, use [WebhookKeyGet](./webhook-key-get.md) — not a CtxSet `from` source. If webhook templates need `${ctx:NAME}`, run CtxSet first and WebhookKeyGet after it.

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

User-controlled values that themselves contain `${...}` are emitted as literal text, so templates are not vulnerable to injection.

### `extract` fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `regex` | String | **Required** | Regex pattern to match against the value |
| `group` | Integer | `1` | Capture group to use (group `0` is the whole match) |

### `transform` fields

Specify at most one transformation operation per rule. `trim: false` does not count as an operation. Additional steps must use a later rule that reads the previous context variable (`from.type: ctx`). Configuring more than one operation in the same `transform` is rejected.

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
| `values` | Object | `{}` | Lookup table mapping input value to output value |
| `default` | String | none | Output when no key matches |

---

## Common configuration scenarios

### 1. Copy a header into a context variable

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

### 2. Build a composite key with a template

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

### 3. Extract, transform, and map a plan tier

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

### 4. Chain: CtxSet feeds a downstream RateLimitLocal key

CtxSet runs in the request phase before RateLimitLocal, so the variable it sets is available to RateLimitLocal through a `ctx` key source (`key: { type: ctx, name: <name> }`). Both plugins consume the unified `KeyGet` accessor.

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

---

## FAQ

### Q1: What happens if the source header is missing?

**A**: The variable is set to `default` if you provide one; otherwise no variable is set for that rule. Downstream plugins then see the variable as absent.

### Q2: Can one rule combine `from`, `extract`, `transform`, and `mapping`?

**A**: Yes. The pipeline runs in order: resolve the source value, then `extract`, then `transform`, then `mapping`. `default` applies if the result is empty.

### Q3: Which downstream plugins can read these variables?

**A**: Any request-phase plugin that runs after CtxSet. RateLimitLocal reads them through its `key` (`{ type: ctx, name: <name> }`), and DSL reads them through the context API. Order matters — put CtxSet before its consumers in the same phase array.

---

## Complete configuration example

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
    - type: RateLimitLocal
      config:
        rate: 100
        window: 60
        burst: 20
        key:
          type: ctx
          name: "tenant"
```

## Related links

- [Webhook KeyGet](./webhook-key-get.md)
- [Rate Limit Plugin](./rate-limit-local.md)
- [DSL Plugin](./dsl/README.md)
- [Filters overview](../README.md)
