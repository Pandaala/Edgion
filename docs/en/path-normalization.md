# Path Normalization

Edgion enforces path normalization at every request entry, eliminating bypass vectors against path-based access control rules (percent-encoding, double slashes, dot-segments, path-parameters). **This feature is always-on; there is no global enable/disable switch.**

---

## Why Normalize?

Typical bypass attack:

- Operator configures `RequestRestriction` plugin to block `/admin` (a `Path` rule with `values: ["/admin"]`)
- Attacker sends `/%61dmin` (percent-encoded form of `/admin`)
- Without normalization: literal string compare `/%61dmin ≠ /admin` → bypass succeeds
- With normalization: `/%61dmin` → `/admin` → matches deny rule → rejected

---

## Two-View Concept

Edgion exposes two path views:

| View | Variables | Meaning |
|------|-----------|---------|
| **current (default)** | `$path` / `$uri` / `req.path` / `req.uri` | Normalized current path; reflects rewrite plugin changes |
| **entry** | `$request_path` / `$request_uri` / `req.request_path` / `req.request_uri` | The raw path the client actually sent; frozen, immutable |

**Core principles**:

- Route matching, ACL decisions, access_log default to the **current view** (normalized)
- Use the **entry view** for audit / security investigation when you need the client's original form

---

## Configuration Options

The `EdgionGatewayConfig` CRD provides two optional hardening switches:

```yaml
spec:
  pathNormalization:
    rejectEscapedSlashes: false           # 400-reject requests containing %2F/%5C
    rejectInvalidPercentEncoding: false   # 400-reject requests with malformed percent-encoding
```

**Note**: There is no `enabled` field — path normalization is mandatory in Edgion and cannot be disabled.

| Option | Default | When to enable |
|--------|---------|----------------|
| `rejectEscapedSlashes` | `false` | High-security environments where literal encoded slashes are never legitimate |
| `rejectInvalidPercentEncoding` | `false` | High-security environments needing protection from overlong UTF-8 / malformed inputs |

### Raw URI Output in Access Log

By default the access log emits only the normalized `path` field — no raw form, no query.
To audit the client's original full URI, enable the gateway-side YAML option:

```yaml
access_log_options:
  includeRawUri: true
```

This adds a `request_uri` field to access log JSON, containing the client entry full raw URI (raw path + raw query). See [Access Log](ops-guide/observability/access-log.md).

**Note**: `access_log_sensitive.mode` defaults to `replace`, so `request_uri` (like `path`) renders as `***` unless the mode is changed. Enabling `includeRawUri` alone no longer yields a readable raw-entry view; see [access_log_sensitive](ops-guide/configuration/gateway-config.md#access_log_sensitive).

---

## Security Commitment Scope

**Edgion guarantees ACL bypass protection in the following cases**:

- `%XX` encoding bypass for unreserved chars (e.g., `/%61dmin`)
- `%2F` / `%5C` encoded slash bypass
- Double slash bypass (`/admin//x`)
- Dot-segment bypass (`/admin/../secret`)
- Path-parameter bypass (`/admin/..;/secret`, CVE-2022-35202 family)

**Out of default commitment scope**:

- Overlong UTF-8 encodings (`%c0%ae%c0%ae`) — enable `rejectInvalidPercentEncoding` for hardening
- Double percent-encoding (`%252E%252E`) — same
- Upstream framework-specific raw URI parsing quirks (outside Edgion's control)

---

## Migration Guide (upgrading to a path-normalization version)

### 1. `req.uri` / `$uri` semantic change

**Old behavior**: returns raw current URI (with the client's original encoded form)

**New behavior**: returns normalized current URI (post-normalization)

If your audit log relies on raw URI:

```yaml
# Old (may have returned raw)
- template: "uri=${uri}"

# New (explicitly request entry raw)
- template: "entry_uri=${requestUri}"
```

### 2. `RegularExpression` path matching

**Existing regex containing the following literals will silently never match** (request side is already normalized):

- `%XX` literals (e.g. `%2F`, `%61`)
- `//` (double slash)
- `/./`, `/../` (dot-segments)
- `..;` (path-parameter bypass form)

Rewrite the regex to operate on the normalized space:

```yaml
# ❌ Silently fails — `/%61dmin` is normalized to `/admin`, this pattern never hits
path:
  type: RegularExpression
  value: "/%61dmin"

# ✅ Correct
path:
  type: RegularExpression
  value: "/admin"
```

### 3. `keyMatch` + `key: { type: path }` configuration

A path-key `keyMatch` condition's `values` are normalized at config load time (so they live in the same normalized space as request-side matching). Things like `/admin/` (trailing slash) are normalized to `/admin`.

If you need literal raw rejection, switch to `keyMatch` + `key: { type: requestPath }`.

### 4. `;param` side effect on allow-mode rules

Path normalization strips `;param=...` from each segment (defends against `..;/` bypass).

- Affects **deny-mode groups** (`mode: Deny`): `/admin/..;jsessionid=x/secret` becomes `/secret` in the ACL view, correctly matching a path `keyMatch` with `values: ["/secret"]` ✓
- Affects **allow-mode groups** (`mode: Allow`): a request `/api/users;v=1/data` becomes `/api/users/data` in the ACL view — if you configured a path `keyMatch` with `values: ["/api/users"]` (exact), it **won't match**

The upstream still receives the raw path with `;param` intact; applications relying on `;jsessionid` are unaffected.

---

## Common Use Cases

### Block admin paths (recommended pattern)

```yaml
- type: RequestRestriction
  config:
    mode: Deny
    conditions:
      anyOf:
        - allOf:
            - type: keyMatch
              key:
                type: path             # current view, covers all encoding variants
              values:
                - "/admin"
                - "/internal"
```

### Audit log records client raw URI

Compute the value with `CtxSet`, then log the resulting ctx var with an
`AccessLogExtern from: ctx` declaration — `AccessLogExtern` is the only
carrier that projects a ctx var into the access log; `CtxSet` alone does not
make it appear there. See [Access Log — Extending the log:
AccessLogExtern](ops-guide/observability/access-log.md#extending-the-log-accesslogextern).

```yaml
- type: CtxSet
  config:
    rules:
      - name: entry_uri
        template: "${requestUri}"   # entry raw URI, frozen
---
apiVersion: edgion.io/v1alpha1
kind: EdgionPlugins
metadata:
  name: audit-fields
spec:
  accessLogExtern:
    - key: entry_uri
      from: ctx
      name: entry_uri
```

---

## What Path Does the Upstream Receive? (v4)

Starting with v4, Edgion forwards the normalized path to the upstream by default — aligning with Envoy / Kong behavior. The decision uses the following priority:

| Priority | Channel | Source | Default |
|---|---|---|---|
| 0 | rewrite plugin already wrote path | `url_rewrite` / `proxy_rewrite` / DSL `req.set_uri` etc. | use rewrite result (terminal) |
| 1 | per-route annotation `edgion.io/forward-raw-path` | HTTPRoute `metadata.annotations` | `"true"` forces raw / `"false"` forces normalized |
| 2 | gateway-to-gateway forwarding (region_route) | `region_route` plugin | raw (avoids double-normalization conflict with peer gateway) |
| 3 | global switch `forward_normalized_path` | `path_normalization` config section | `true` (normalized) |
| Default | fallthrough | — | normalized |

When an auth plugin removes a credential from the query string, it changes only
the query. The path and the original HTTP/1 request-target form, including an
absolute-form target, remain available to the forwarding decision above.
For HTTP/2, the request's scheme and authority remain intact.
Path normalization and path-only rewrite plugins also retain HTTP/2 scheme
and authority in the request URI. An explicit absolute URI rewrite replaces
those URI parts; the final wire authority still follows the upstream Host
reconciliation and configured upstream authority policy.

### Compared with Envoy / Kong

| Gateway | Default upstream view | Toggleable? |
|---|---|---|
| Envoy `normalize_path` | normalized | yes |
| Kong Traditional router | normalized | yes |
| **Edgion v4** | **normalized** (industry-aligned) | yes (`forward_normalized_path: false`) |
| Edgion v3.1 and earlier | raw | — |

### Upgrade Notes

If your upstream is sensitive to literal path form (e.g., your application has special routing logic for `%2F` literals), the v4 default will surface a behavior change — the upstream now receives the decoded form. Two solutions:

1. **per-route opt-out**: add the `edgion.io/forward-raw-path: "true"` annotation to the affected HTTPRoute
2. **global rollback**: set `forward_normalized_path: false` under `path_normalization` (affects all routes)

### `;param` Compatibility Scenario

If your upstream application depends on path-parameters (like `;jsessionid=xxx`), enable:

```yaml
path_normalization:
  preserve_path_parameters: true
```

**Note**: enabling this opt-out removes the `..;/` style bypass from Edgion's default protection scope (see spec §14.4). It is recommended to additionally use `keyMatch` + `key: { type: requestPath }` regex conditions to reject suspicious patterns containing `;`.

---

## Related Documents

- [EdgionGatewayConfig](ops-guide/configuration/edgion-gateway-config.md) — `spec.pathNormalization` configuration reference
- RequestRestriction plugin: [user-guide/http-route/filters/edgion-plugins/request-restriction.md](user-guide/http-route/filters/edgion-plugins/request-restriction.md)
