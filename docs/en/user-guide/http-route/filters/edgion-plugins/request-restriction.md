# Request Restriction Plugin

## Overview

The `RequestRestriction` plugin restricts access based on request attributes, supporting multiple data sources and matching methods. Unlike the `IpRestriction` plugin which focuses on IP addresses, this plugin supports access control based on Header, Cookie, Query parameters, Path, Method, and Referer.

## Features

- **Multiple data sources**: Header, Cookie, Query, Path, Method, Referer
- **Exact and regex matching**: `allow`/`deny` for exact matching, `allowRegex`/`denyRegex` for regex matching
- **Automatic performance optimization**: Exact match lists exceeding 16 values automatically use HashSet (O(1) lookup)
- **Flexible rule composition**: Supports Allow/Deny lists with Deny having the highest priority
- **Missing value handling**: Configurable behavior when the target value is missing (Allow/Deny/Skip)
- **Multi-rule modes**: Any (deny on any rule trigger) or All (deny only when all rules trigger)
- **Custom responses**: Configurable status code and message on denial

## Difference from Plugin Conditions

| Dimension | Plugin Conditions | Request Restriction |
|-----------|------------------|---------------------|
| **Purpose** | Conditional execution framework, decides whether to run a plugin | Standalone plugin, directly rejects non-compliant requests |
| **Return behavior** | Skips the plugin (continues processing chain) | Rejects the request (returns error response) |
| **Configuration location** | Attached to other plugins as conditions | Configured as a standalone plugin |
| **Use cases** | Canary releases, A/B testing, conditional execution | Access control, security protection |

## Configuration Parameters

### RequestRestrictionConfig

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `rules` | `RestrictionRule[]` | ✅ | - | List of restriction rules |
| `matchMode` | `string` | ❌ | `Any` | Rule matching mode: `Any` (any triggers) or `All` (all must trigger) |
| `status` | `integer` | ❌ | `403` | HTTP status code on denial |
| `message` | `string` | ❌ | `Access denied...` | Response message on denial |

### RestrictionRule

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | `string` | ❌ | - | Rule name (for logging) |
| `source` | `string` | ✅ | - | Data source: `Header`, `Cookie`, `Query`, `Path`, `Method`, `Referer` |
| `key` | `string` | Depends | - | Key name (required for Header/Cookie/Query, ignored for Path/Method/Referer) |
| `allow` | `string[]` | ❌ | - | Exact match allowlist |
| `allowRegex` | `string[]` | ❌ | - | Regex match allowlist |
| `deny` | `string[]` | ❌ | - | Exact match denylist (highest priority) |
| `denyRegex` | `string[]` | ❌ | - | Regex match denylist |
| `caseSensitive` | `boolean` | ❌ | `true` | Whether exact matching is case-sensitive |
| `onMissing` | `string` | ❌ | `Allow` | Behavior when value is missing: `Allow`, `Deny`, `Skip` |

> **Note**: At least one of `allow`, `allowRegex`, `deny`, `denyRegex` must be configured.

### Data Source Reference

| Data Source | Description | Requires key |
|-------------|-------------|-------------|
| `Header` | HTTP request header | ✅ |
| `Cookie` | HTTP Cookie | ✅ |
| `Query` | URL query parameter | ✅ |
| `Path` | Request path | ❌ |
| `Method` | HTTP method | ❌ |
| `Referer` | Referer request header | ❌ |

### Matching Methods

| Configuration | Matching Method | Description | Example |
|---------------|----------------|-------------|---------|
| `allow`/`deny` | Exact match | Full string equality | `"GET"`, `"/health"` |
| `allowRegex`/`denyRegex` | Regex match | Using regular expressions | `"(?i).*Bot.*"`, `"^/api/v[0-9]+/.*"` |

> **Tip**: Use the `(?i)` prefix in regex for case-insensitive matching, e.g., `(?i).*bot.*`.

## Execution Logic

```
┌─────────────────────────────────────────────────────────┐
│                  Request Restriction                     │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  1. Iterate through all rules                            │
│     │                                                    │
│     ├─ Get value from data source                        │
│     │   └─ Header[key] / Cookie[key] / Query[key]        │
│     │   └─ Path / Method / Referer                       │
│     │                                                    │
│     ├─ Missing value handling (onMissing)                 │
│     │   ├─ Allow → rule allows                           │
│     │   ├─ Deny → rule denies                            │
│     │   └─ Skip → skip this rule                         │
│     │                                                    │
│     ├─ Deny check (highest priority)                     │
│     │   └─ Matches deny/denyRegex list → rule denies     │
│     │                                                    │
│     └─ Allow check                                       │
│         ├─ Allow list exists but no match → rule denies   │
│         └─ Matches allow/allowRegex list → rule allows    │
│                                                          │
│  2. Aggregate rule results                               │
│     ├─ matchMode=Any: any rule denies → return error     │
│     └─ matchMode=All: all rules deny → return error      │
│                                                          │
│  3. All rules pass → continue processing chain           │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

## Use Cases

### 1. Block Crawlers and Bots

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: block-bots
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        rules:
          - name: "block-bots"
            source: Header
            key: "User-Agent"
            denyRegex:
              - "(?i).*Bot.*"
              - "(?i).*Spider.*"
              - "(?i).*Crawler.*"
              - "(?i).*curl.*"
              - "(?i).*wget.*"
            onMissing: Deny  # Also deny requests without UA
        status: 403
        message: "Bot access denied"
```

### 2. Path Allowlist (API Paths Only)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-whitelist
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        rules:
          - name: "api-only"
            source: Path
            allow:
              - "/health"
              - "/ready"
            allowRegex:
              - "^/api/.*"
        status: 404
        message: "Not found"
```

### 3. Read-Only API (GET/HEAD Only)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: readonly-api
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        rules:
          - name: "readonly"
            source: Method
            allow:
              - "GET"
              - "HEAD"
              - "OPTIONS"
        status: 405
        message: "Method not allowed"
```

### 4. Require Authentication Header

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: require-auth
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        rules:
          - name: "require-auth"
            source: Header
            key: "X-Auth-Token"
            deny:
              - "invalid"
              - "expired"
            onMissing: Deny  # This header is required
        status: 401
        message: "Authentication required"
```

### 5. Referer Restriction (Hotlink Protection)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: referer-check
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        rules:
          - name: "allow-internal"
            source: Referer
            allowRegex:
              - ".*example\\.com.*"
              - ".*example\\.org.*"
            onMissing: Deny
        status: 403
        message: "Invalid referer"
```

### 6. Block Debug Parameters

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: block-debug
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        rules:
          - name: "block-debug-query"
            source: Query
            key: "debug"
            deny:
              - "true"
              - "1"
          - name: "block-debug-cookie"
            source: Cookie
            key: "debug"
            deny:
              - "true"
              - "1"
        matchMode: Any  # Deny on any rule trigger
        status: 403
        message: "Debug mode not allowed in production"
```

### 7. Comprehensive Security Policy

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: security-policy
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        matchMode: Any
        rules:
          # Block crawlers
          - name: "block-bots"
            source: Header
            key: "User-Agent"
            denyRegex:
              - "(?i).*Bot.*"
              - "(?i).*Spider.*"
          # Block sensitive paths
          - name: "block-sensitive"
            source: Path
            denyRegex:
              - ".*/admin/.*"
              - ".*/internal/.*"
              - ".*\\.php$"
              - ".*\\.asp$"
          # Block SQL injection attempts
          - name: "block-sqli"
            source: Query
            key: "id"
            denyRegex:
              - ".*('|--|;|/\\*).*"
        status: 403
        message: "Access denied by security policy"
```

## Comparison with Other Gateways

| Feature | APISIX | Kong | Edgion |
|---------|--------|------|--------|
| **User-Agent restriction** | ✅ ua-restriction | ✅ bot-detection | ✅ RequestRestriction |
| **Path/URI restriction** | ✅ uri-blocker | ❌ | ✅ RequestRestriction |
| **Referer restriction** | ✅ referer-restriction | ❌ | ✅ RequestRestriction |
| **Cookie restriction** | ❌ | ❌ | ✅ RequestRestriction |
| **Query restriction** | ❌ | ❌ | ✅ RequestRestriction |
| **Method restriction** | ✅ consumer-restriction | ❌ | ✅ RequestRestriction |
| **Unified plugin** | ❌ (multiple separate) | ❌ (multiple separate) | ✅ (single plugin) |

Edgion's `RequestRestriction` plugin unifies multiple restriction capabilities into a single plugin, resulting in simpler configuration and easier maintenance.

## Notes

1. **Rule order**: Rules execute in configuration order, but with `matchMode=Any`, processing stops immediately after the first deny rule triggers
2. **Deny takes priority**: Within a single rule, deny/denyRegex lists have higher priority than allow/allowRegex lists
3. **Case sensitivity**: Exact matching is case-sensitive by default; use `caseSensitive: false` to disable; for regex, use the `(?i)` prefix
4. **Regex performance**: Multiple regex patterns are merged into a single regex (joined with `|`), reducing matching overhead
5. **HashSet optimization**: Exact match lists exceeding 16 values automatically use HashSet for O(1) lookup
6. **Missing value handling**: Choose the appropriate `onMissing` behavior based on security requirements — authentication scenarios typically use `Deny`

## Related Plugins

- [IpRestriction](./ip-restriction.md) - IP-based access control
- [JwtAuth](./jwt-auth.md) - JWT authentication
- [BasicAuth](./basic-auth.md) - Basic authentication
