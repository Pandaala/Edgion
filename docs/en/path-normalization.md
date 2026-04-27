# Path Normalization

> Detailed design spec: `docs/superpowers/specs/2026-04-26-uri-normalization-design.md`

Edgion enforces path normalization at every request entry, eliminating bypass vectors against path-based access control rules (percent-encoding, double slashes, dot-segments, path-parameters). **This feature is always-on; there is no global enable/disable switch.**

---

## Why Normalize?

Typical bypass attack:

- Operator configures `RequestRestriction` plugin with `deny: ["/admin"]`
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
To audit the client's original full URI, enable the gateway-side TOML option:

```toml
[access_log_options]
include_raw_uri = true
```

This adds a `request_uri` field to access log JSON, containing the client entry full raw URI (raw path + raw query). See `skills/03-coding/observability/00-access-log.md`.

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

### 3. `RestrictionSource::Path` configuration

The `Path` view's `deny` / `allow` lists are normalized at config load time (so they live in the same normalized space as request-side matching). Things like `/admin/` (trailing slash) are normalized to `/admin` — an INFO log records the transformation, never silent.

If you need literal raw rejection, switch to `RestrictionSource::RequestPath`.

### 4. `;param` side effect on allow-list rules

Path normalization strips `;param=...` from each segment (defends against `..;/` bypass).

- Affects **deny rules**: `/admin/..;jsessionid=x/secret` becomes `/secret` in the ACL view, correctly matching `deny: /secret` ✓
- Affects **allow rules**: a request `/api/users;v=1/data` becomes `/api/users/data` in the ACL view — if you configured `allow: /api/users` (exact), it **won't match**

The upstream still receives the raw path with `;param` intact; applications relying on `;jsessionid` are unaffected.

---

## Common Use Cases

### Block admin paths (recommended pattern)

```yaml
- type: RequestRestriction
  config:
    rules:
      - name: block-admin
        source: Path                # current view, covers all encoding variants
        deny:
          - "/admin"
          - "/internal"
        onMissing: Allow
```

### Audit log records client raw URI

```yaml
- type: CtxSet
  config:
    rules:
      - name: entry_uri
        template: "${requestUri}"   # entry raw URI, frozen
```

---

## What Path Does the Upstream Receive? (v4)

Starting with v4, Edgion forwards the normalized path to the upstream by default — aligning with Envoy / Kong behavior. The decision uses the following priority:

| Priority | Channel | Source | Default |
|---|---|---|---|
| 0 | rewrite plugin already wrote path | `url_rewrite` / `proxy_rewrite` / DSL `req.set_uri` etc. | use rewrite result (terminal) |
| 1 | per-route annotation `edgion.io/forward-raw-path` | HTTPRoute `metadata.annotations` | `"true"` forces raw / `"false"` forces normalized |
| 2 | gateway-to-gateway forwarding (region_route) | `region_route` plugin | raw (avoids double-normalization conflict with peer gateway) |
| 3 | global switch `forward_normalized_path` | `[path_normalization]` config section | `true` (normalized) |
| Default | fallthrough | — | normalized |

### Compared with Envoy / Kong

| Gateway | Default upstream view | Toggleable? |
|---|---|---|
| Envoy `normalize_path` | normalized | yes |
| Kong Traditional router | normalized | yes |
| **Edgion v4** | **normalized** (industry-aligned) | yes (`forward_normalized_path = false`) |
| Edgion v3.1 and earlier | raw | — |

### Upgrade Notes

If your upstream is sensitive to literal path form (e.g., your application has special routing logic for `%2F` literals), the v4 default will surface a behavior change — the upstream now receives the decoded form. Two solutions:

1. **per-route opt-out**: add the `edgion.io/forward-raw-path: "true"` annotation to the affected HTTPRoute
2. **global rollback**: set `forward_normalized_path = false` under `[path_normalization]` (affects all routes)

### `;param` Compatibility Scenario

If your upstream application depends on path-parameters (like `;jsessionid=xxx`), enable:

```toml
[path_normalization]
preserve_path_parameters = true
```

**Note**: enabling this opt-out removes the `..;/` style bypass from Edgion's default protection scope (see spec §14.4). It is recommended to additionally use `RestrictionSource::RequestPath` regex rules to reject suspicious patterns containing `;`.

---

## Related Documents

- Design spec: `docs/superpowers/specs/2026-04-26-uri-normalization-design.md`
- Configuration reference: `skills/02-features/02-config/02-edgion-gateway-config.md` (`spec.pathNormalization`)
- RequestRestriction plugin: [user-guide/http-route/filters/edgion-plugins/request-restriction.md](user-guide/http-route/filters/edgion-plugins/request-restriction.md)
