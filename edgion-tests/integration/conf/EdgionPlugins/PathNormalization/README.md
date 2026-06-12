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
| `01_EdgionPlugins_request-restriction-path-deny.yaml` | `EdgionPlugins/path-norm-request-restriction-deny` | RequestRestriction with `Path` + `RequestPath` deny rules (default strip mode) |
| `HTTPRoute_default_path-normalization.yaml` | `HTTPRoute/path-norm-route` | Routes for scenario 01 (default strip mode) |
| `PreserveParam/02_EdgionPlugins_preserve-path-parameters.yaml` | `EdgionPlugins/path-norm-preserve-param` | RequestRestriction for scenario 02 (preserve_path_parameters opt-out) |
| `PreserveParam/HTTPRoute_preserve-path-parameters.yaml` | `HTTPRoute/path-norm-preserve-route` | Routes for scenario 02 (preserve opt-out, host: `path-norm-preserve.example.com`) |
| `ForwardView/03_HTTPRoute_default-normalized-forward.yaml` | `HTTPRoute/forward-view-default` | Default (no annotation) route for scenario 03, host: `path-fwd-default.example.com` |
| `ForwardView/03_HTTPRoute_annotation-raw-true.yaml` | `HTTPRoute/forward-view-raw` | `forward-raw-path: "true"` route for scenario 03, host: `path-fwd-raw.example.com` |
| `ForwardView/03_HTTPRoute_annotation-norm-false.yaml` | `HTTPRoute/forward-view-norm-override` | `forward-raw-path: "false"` route for scenario 03, host: `path-fwd-norm.example.com` |

### Scenario 01: Default strip mode (`preserve_path_parameters = false`)

Item: `EdgionPlugins/PathNormalization` — runs with default gateway config.

### Scenario 02: preserve_path_parameters opt-out (`preserve_path_parameters = true`)

Item: `EdgionPlugins/PathNormalization/PreserveParam`

### Scenario 03: Upstream forwarding decision (ForwardView)

Item: `EdgionPlugins/PathNormalization/ForwardView`

Tests the spec §14 single-point upstream forwarding decision: per-route
`edgion.io/forward-raw-path` annotation tri-state (`"true"` force raw / `"false"` force
normalized / absent default flow). Uses the **default** gateway YAML config — no special
`path_normalization` settings required.

The global `forward_normalized_path` rollback switch is documented in Scenario 04 below
and requires a gateway restart with custom YAML; the region_route default-raw behavior is
documented in Scenario 05. See
`docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14 for the full decision
matrix.

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

## Scenario 03: ForwardView — Upstream Path Forwarding Decision

Item: `EdgionPlugins/PathNormalization/ForwardView`

Tests which path representation the gateway forwards to upstream for a percent-encoded
request (`/%61pi/data`, where `%61` = `a`). Route matching always uses the normalized
path; only the forwarding view differs.

### TC-A: Default (no annotation) — upstream receives normalized path

```bash
curl -i -H "Host: path-fwd-default.example.com" \
     "http://localhost:31180/%61pi/data?x=1"
# Expected: backend echoes Path: /api/data (normalized; %61 decoded to 'a')
# Query string ?x=1 passes through unchanged.
```

### TC-B: Annotation `forward-raw-path: "true"` — upstream receives raw path

```bash
curl -i -H "Host: path-fwd-raw.example.com" \
     "http://localhost:31180/%61pi/data?x=1"
# Expected: backend echoes Path: /%61pi/data (raw, percent-encoding preserved)
# Route matching still uses normalized /api/data to hit PathPrefix /api.
```

### TC-C: Annotation `forward-raw-path: "false"` — upstream receives normalized path

```bash
curl -i -H "Host: path-fwd-norm.example.com" \
     "http://localhost:31180/%61pi/data?x=1"
# Expected: backend echoes Path: /api/data (annotation forces normalized forwarding)
# This annotation also overrides region_route's preserve_raw_path = true if configured.
```

### TC-D: URLRewrite filter wins over annotation (spec §14 channel 0 priority)

Route has BOTH a `URLRewrite` filter (replaces `/api` → `/rewritten`) AND
`edgion.io/forward-raw-path: "true"` annotation. When the rewrite filter executes,
it sets `ctx.uri_finalized = true`, which causes the single-point decision to skip
entirely — the rewrite result is the terminal form, annotation is ignored.

```bash
curl -i -H "Host: path-fwd-rewrite.example.com" \
     "http://localhost:31180/%61pi/data?q=1"
# Expected: backend echoes Path: /rewritten/data (rewrite wins)
# NOT /%61pi/data (annotation alone would have produced this)
# NOT /api/data (single-point decision alone would have produced this)
```

### TC-E: Multi-step normalization chains end-to-end to upstream

Single request triggers all algorithm steps in sequence: percent-decode +
merge_slashes + dot-segment. All three steps must propagate to the upstream
under default forwarding mode.

```bash
curl -i -H "Host: path-fwd-default.example.com" \
     "http://localhost:31180/%61pi//foo/./bar?q=1"
# Expected: backend echoes Path: /api/foo/bar (?q=1 preserved)
# Demonstrates: %61→'a' (decode) + //→/ (merge) + /./→/ (dot-segment) all chain
```

### TC-F: Invalid annotation value falls through to default flow

Annotation value `"yes"` is not in the strict tri-state set (`"true"` / `"false"`
only). The controller logs INFO and treats invalid values as None → default flow
applies (normalized forwarding).

```bash
curl -i -H "Host: path-fwd-invalid.example.com" \
     "http://localhost:31180/%61pi/data?x=1"
# Expected: backend echoes Path: /api/data (normalized — same as no-annotation TC-A)
# NOT raw /%61pi/data which would mean 'yes' was misinterpreted as truthy
```

### Non-automatable scenarios (require gateway YAML restart)

**Scenario 04**: Global `forward_normalized_path = false` in gateway YAML — all routes
forward raw by default.

```yaml
# gateway YAML config
path_normalization:
  forward_normalized_path: false
```

```bash
# With forward_normalized_path = false:
curl -i -H "Host: path-fwd-default.example.com" \
     "http://localhost:31180/%61pi/data?x=1"
# Expected: backend echoes Path: /%61pi/data (global default is now raw)
```

Per-route annotation `forward-raw-path: "false"` overrides this global flag:

```bash
# With forward_normalized_path = false but annotation "false" on route:
curl -i -H "Host: path-fwd-norm.example.com" \
     "http://localhost:31180/%61pi/data?x=1"
# Expected: backend echoes Path: /api/data (annotation wins over global default)
```

**Scenario 05**: `region_route` plugin with no `forward-raw-path` annotation — upstream
receives whichever path the global `forward_normalized_path` setting dictates.
To verify, attach a region_route plugin to a route that has no `forward-raw-path`
annotation and observe that the region_route plugin's routing decision is independent
of the upstream forwarding view. See `edgion-tests/integration/conf/EdgionPlugins/RegionRoute/`
for the region_route test setup.

---

## Notes on Global-Only Config Options

Two options in `path_normalization` are **global gateway config** (gateway YAML config) and
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
kubectl apply -f edgion-tests/integration/conf/EdgionPlugins/PathNormalization/

# Then run the curl commands above against the gateway's HTTP listener.
# Adjust the port if your gateway is not on 31180.
```
