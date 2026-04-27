# PathNormalization Integration Tests

Tests end-to-end URI path normalization behavior: percent-encoding decode, double-slash collapse,
dot-segment removal, and path-parameter stripping (`;param`). Covers both the routing layer
(HTTPRoute matching on normalized path) and the `RequestRestriction` plugin (Path vs RequestPath
source semantics).

**Host:** `path-norm.example.com`  
**Gateway port:** `31180` (reuses `edgion-plugins-gateway`)

---

## Files

| File | Resource | Purpose |
|---|---|---|
| `01_EdgionPlugins_request-restriction-path-deny.yaml` | `EdgionPlugins/path-norm-request-restriction-deny` | RequestRestriction with `Path` + `RequestPath` deny rules |
| `HTTPRoute_default_path-normalization.yaml` | `HTTPRoute/path-norm-route` | Routes wiring the plugin and testing match types |

---

## Test Cases

### TC-1: `RequestRestriction` Path source blocks encoded `/admin` bypass

The `Path` deny rule targets `/admin` in **normalized** path space. Requests like `/%61dmin` are
normalized to `/admin` before the plugin evaluates them, so the deny fires regardless of encoding.

```bash
# Plain /admin — should be 403
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/test/restriction/admin
# Expected: HTTP/1.1 403

# Percent-encoded a (%61 = 'a') — should also be 403 (normalization decodes to /admin)
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/test/restriction/%61dmin
# Expected: HTTP/1.1 403

# Double-slash bypass attempt — should be 403 (//admin collapsed to /admin)
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/test/restriction//admin"
# Expected: HTTP/1.1 403

# Safe path — no deny match, should reach backend
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/test/restriction/safe
# Expected: HTTP/1.1 200
```

### TC-2: `RequestRestriction` RequestPath source — literal `%2F` raw match

The `RequestPath` deny rule targets the **entry-time raw path** (frozen, not normalized). The
`denyRegex: ["%2[fF]"]` matches any request whose raw (un-decoded) path contains `%2F` or `%2f`.
This is an audit-style rule — the regex is never normalized.

```bash
# Raw path contains %2F — should be 403 (literal raw match)
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/test/restriction/admin%2Fextra"
# Expected: HTTP/1.1 403

# Same with lowercase %2f — should be 403
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/test/restriction/admin%2fextra"
# Expected: HTTP/1.1 403

# Path with %61 (letter 'a') — no %2F, so RequestPath rule does NOT fire.
# However, Path deny rule fires because %61dmin normalizes to admin.
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/test/restriction/%61dmin
# Expected: HTTP/1.1 403 (caught by Path deny, not RequestPath deny)

# Path with no special encoding — RequestPath rule does not fire
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/test/restriction/safe
# Expected: HTTP/1.1 200
```

### TC-3: HTTPRoute `Exact: /admin` matches `/%61dmin` (normalization before routing)

The HTTPRoute rule uses `Exact: /admin`. URI normalization decodes `%61` → `a` before the route
matcher runs, so `/%61dmin` is treated as `/admin` and matches.

```bash
# Encoded path — should match Exact /admin route and reach backend
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/%61dmin
# Expected: HTTP/1.1 200

# Plain path — same route, obviously matches
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/admin
# Expected: HTTP/1.1 200
```

### TC-4: HTTPRoute `PathPrefix: /api` matches `/api//v1//users` (double-slash collapse)

Double slashes are collapsed to a single slash during normalization (`merge_slashes`), so
`/api//v1//users` becomes `/api/v1/users`, which matches `PathPrefix: /api`.

```bash
# Double slash in path — should be collapsed and match /api prefix
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/api//v1//users"
# Expected: HTTP/1.1 200

# Triple slash — also collapsed
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/api///v1/users"
# Expected: HTTP/1.1 200

# Plain path — matches without normalization needed
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/api/v1/users
# Expected: HTTP/1.1 200
```

### TC-5: HTTPRoute `RegularExpression: ^/regex/v[0-9]+/.*$` matches on normalized path

Regular expressions are evaluated against the **normalized** path. `%2F` is decoded to `/` before
matching, so `/regex%2Fv1%2Fusers` normalizes to `/regex/v1/users` and matches the pattern.

Note: Operators must write regex patterns against the normalized path form. A regex containing
literal `%2F` (e.g. `^/regex/v[0-9]+%2F`) would **never match** because the request path has
already been decoded before matching.

```bash
# Plain path — matches directly
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/regex/v1/users
# Expected: HTTP/1.1 200

# %2F decoded to / — normalizes to /regex/v1/users, matches pattern
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/regex%2Fv1%2Fusers"
# Expected: HTTP/1.1 200

# Encoded version character — %76 = 'v', normalizes to /regex/v1/users
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/regex/%761/users"
# Expected: HTTP/1.1 200

# Non-matching path (letter instead of digit after 'v') — 404 or unmatched
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/regex/va/users
# Expected: HTTP/1.1 404 (no route matches)
```

### TC-6: Path-parameter bypass (`..;jsessionid`) — `strip_path_parameters` (A3)

Path normalization strips the `;param` portion from each path segment before applying dot-segment
removal. This neutralizes Tomcat-style `..;/` traversal attacks.

```bash
# Path-parameter traversal — ;jsessionid stripped, then /../ resolved to /secret
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/admin/..;jsessionid=abc/secret"
# Expected: HTTP/1.1 200 (normalized to /secret, matches Exact /secret rule)

# Variant with arbitrary param name
curl -i -H "Host: path-norm.example.com" \
     "http://localhost:31180/admin/..;x=1/secret"
# Expected: HTTP/1.1 200

# Plain /secret — same Exact rule, always reachable
curl -i -H "Host: path-norm.example.com" \
     http://localhost:31180/secret
# Expected: HTTP/1.1 200
```

---

## Notes on Global-Only Config Options

Two options in `[path_normalization]` are **global gateway config** (`EdgionGatewayConfig`) and
cannot be toggled per-scenario without a dedicated gateway instance:

- `reject_escaped_slashes = true` — returns 400 for requests whose raw path contains `%2F`/`%5C`
- `reject_invalid_percent_encoding = true` — returns 400 for requests with non-UTF-8 or malformed
  percent-encoding (captures overlong UTF-8 like `%c0%ae`)

These are **not exercised** in the scenarios above because the shared `edgion-plugins-gateway` uses
the default config (`false` for both). To test them, a separate gateway instance with the relevant
config would be required — outside the scope of this scenario set.

The `RequestPath` deny rule in TC-2 (`denyRegex: ["%2[fF]"]`) is an **application-layer**
substitute that achieves a similar audit effect for `%2F` patterns without requiring a gateway
restart, and is testable in this shared setup.

---

## Running the Tests

```bash
# Deploy scenario resources
kubectl apply -f examples/test/conf/EdgionPlugins/PathNormalization/

# Then run the curl commands above against the gateway's HTTP listener.
# Adjust the port if your gateway is not on 31180.
```
