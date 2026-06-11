---
name: uri-normalization
description: Authoritative document for URI normalization architecture — full-stack naming table, dual-view concept, iron rules, security guarantee scope, set_upstream_uri integration point.
---

# URI normalization (Path Normalization)

> Authoritative source: `docs/superpowers/specs/2026-04-26-uri-normalization-design.md`
> Related review rules: [../../04-review/uri-normalization/SKILL.md](../../../04-review/uri-normalization/SKILL.md)

Edgion enforces path normalization at every request entry (cannot be disabled), eliminating gaps where path-class ACLs could be bypassed via percent-encoding / double slashes / dot-segments / path-parameters.

---

## Concept: dual-view architecture

| View | Reflects | Affected by rewrite plugins | Typical use |
|------|----------|------------------------------|-------------|
| **current view** (default name) | post-rewrite current path/uri, normalized | Yes (the adapter automatically refreshes inside `set_upstream_uri`) | Route matching, ACL decisions, default access_log |
| **entry view** (`request_` prefix) | raw form at the client entry, **frozen** | No | Audit, logging, special raw-deny scenarios |

**Conceptual layering**:

```
operator perspective: $path / $uri / $query           (current view)
                      $request_path / $request_uri    (entry view)
                      req.path / req.uri / ...        (DSL distinguishes likewise)
                       ↑ template/DSL surfaces all delegate to the session trait

session trait:        get_path() -> &str              get_request_path() -> &str
                      get_uri() -> String             get_request_uri() -> String
                      get_query() -> Option<String>   get_query_param(name) -> Option<String>
                       ↑ plugin interface              (single-key, decoded, frozen)

EdgionHttpContext canonical API:
                      ctx.path()                     (current view, read from cache)
                      ctx.request_path()             (entry view, frozen)
                      ctx.query(req)                 (current view, whole-string passthrough)
                      ctx.cached_query(raw_query)    (entry view, single-key decoded, init-once)
                      ctx.uri(req)                   (current path + current query)
                      ctx.request_uri()              (entry path + entry query)
                      ctx.refresh_normalized_path(req)(refresh current cache;
                                                       called internally by session_adapter::set_upstream_uri)
                       ↑ single source of truth

pingora field:        req_header().uri
                       ↑ business code MUST NOT read directly (allowlist below "iron rules and allowlist")
```

---

## Full-stack naming table (§4 authoritative copy)

| Layer | current view | entry view (frozen) |
|-------|--------------|----------------------|
| **EdgionHttpContext methods** (canonical) | `ctx.path() -> &str` | `ctx.request_path() -> &str` |
| | `ctx.uri(req) -> String` | `ctx.request_uri() -> String` |
| | `ctx.query(req) -> Option<&str>` (whole, raw, passthrough) | `ctx.cached_query(entry_raw_query) -> &HashMap<String,String>` (single-key, decoded, frozen) |
| **EdgionHttpContext write methods** (only used by the adapter) | `ctx.refresh_normalized_path(req)` | — |
| **pingora raw fields** (business code MUST NOT read directly; allowlist below) | — | `req_header().uri` |
| **session trait** | `get_path() -> &str` | `get_request_path() -> &str` |
| | `get_uri() -> String` | `get_request_uri() -> String` |
| | `get_query() -> Option<String>` (whole, raw) | `get_query_param(name) -> Option<String>` (single-key, decoded, frozen) |
| **DSL builtin** | `req.path` | `req.request_path` |
| | `req.uri` | `req.request_uri` |
| | `req.query_string` (whole, raw) | `req.query("name")` (single-key, decoded, frozen) |
| **ctx_set template variable** | `$path` | `$request_path` |
| | `$uri` | `$request_uri` |
| | (whole-query template var not supported) | `TemplateVarType::Query` with `var_name` (single-key, decoded, frozen) |
| **`request_restriction` source** | `Path` | `RequestPath` |
| | (whole-query source not supported) | `Query` with `key` (single-key, decoded, frozen) |

### Naming rules

- Default name (`path` / `uri`) = the current normalized view (cached, reflects rewrite).
- `request_` prefix = entry raw view (frozen, unaffected by rewrite).
- query is not normalized; whole-query access uses a single name (`query`).

### query view: whole-string vs single-key

The query string has two access shapes that follow different views:

| Access shape | View | Encoding | Source |
|--------------|------|----------|--------|
| Whole string (`session.get_query()` / `req.query_string`) | current | raw (no decode) | `req_header().uri.query()` (live) |
| Single key (`session.get_query_param(name)` / `req.query("name")` / `ctx_set TemplateVarType::Query` / `request_restriction` Query source) | **entry (frozen)** | **percent-decoded** (`+` → space) | `entry_raw_query` via `EdgionHttpContext::cached_query` |

Single-key accessors share one per-request `OnceLock<HashMap>` cache (`EdgionHttpContext::cached_query_cell`) seeded from `entry_raw_query`. Result: the route matcher and every plugin reading by key see the same decoded value, regardless of read order or any in-request `set_upstream_uri` rewrite.

### query view asymmetry (whole-string only — partially closed)

`$request_query` / `req.request_query_string` (a whole-string entry view) still does not exist. The entry whole query is only available indirectly via `$request_uri` — split it from there. ROI of adding such a variable is low (most audits care about the full URI). YAGNI.

Single-key entry-view access **does** exist (`get_query_param(name)` / `req.query("name")`); it has been entry-frozen and decoded since the unified-query-string-cache landing (2026-05-10).

---

## Iron rules and allowlist

**Iron rule**: business code MUST NOT read `req_header().uri.path()` directly; it MUST use `ctx.path()` or `session.get_path()`.

Allowlist (direct reads at the following sites are legitimate):

- The ctx path/uri method implementations themselves in `src/core/gateway/ctx.rs`.
- The entry-snapshot block in `pg_request_filter.rs::build_request_metadata` (the `raw_path` / `raw_query` capture feeding the path-normalization entry layer).
- ACME / well-known protocol handling code.
- Pingora default forwarding logic paths.
- The session trait implementations in `session_adapter.rs`.

---

## `set_upstream_uri` integration point

`session_adapter.rs:443` is the **only** site in the entire codebase that writes to `req_header_mut().set_uri()`. All rewrite plugins (`proxy_rewrite`, `url_rewrite`, `request_redirect`, DSL `req.set_uri`, etc.) go through the trait wrapper:

```rust
// session_adapter.rs:443 (after modification)
fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()> {
    let parsed_uri = uri.parse::<Uri>().map_err(|e| Box::new(e) as PluginSessionError)?;
    self.inner.req_header_mut().set_uri(parsed_uri);
    self.ctx.refresh_normalized_path(self.inner.req_header());  // newly added line
    Ok(())
}
```

**Plugin layer requires zero changes**: all existing rewrite plugins keep their original code, and trait calls automatically get ctx cache synchronization.

Effects after rewrite:

- `ctx.path()` / `$uri` / `req.uri` → reflects the **new path** (cache refreshed).
- `ctx.request_path()` / `$request_uri` → still reflects the **entry raw form** (entry view frozen).
- The upstream receives the rewritten path.

---

## Security guarantee scope (§1.4 summary)

ACL bypass protection **holds for** the following cases (within the guarantee scope):

- `%XX`-form encoded unreserved characters / `%2F` / `%5C` encoded bypass.
- Double-slash bypass (`/admin//x` etc.).
- Standard dot-segment bypass (`/admin/../secret`).
- Path-parameter bypass (`/admin/..;/secret` etc., similar to CVE-2022-30065 / 35202).
- Encoded dot-segment combinations (`/admin/%2E%2E/secret`).

**Outside the guarantee scope** (no guarantees):

- Overlong UTF-8 encoded traversal (`%c0%ae%c0%ae` IIS-style) — encoded form is preserved without expansion by default; for high-security scenarios enable `reject_invalid_percent_encoding`.
- Double percent-encoding (`%252E%252E`) — only one decode pass by default; same toggle for high-security scenarios.
- Non-RFC-3986-compliant upstreams that parse raw URI specially.

Recommended for high-security scenarios:

```yaml
path_normalization:
  reject_escaped_slashes: true          # reject requests that contain %2F / %5C
  reject_invalid_percent_encoding: true # reject requests with non-UTF-8 / invalid hex percent-encoding
```

See spec §1.4 for the full security guarantee scope.

---

## Normalization algorithm (Edgion BASE, fixed, not individually toggleable)

Steps run in this fixed order:

1. **strip_path_parameters**: strip `;param=...` from each path segment (defends against `..;/`-class bypass).
2. **percent-decode** (decode according to the allowlist).
3. **merge_slashes** (collapse consecutive `/` into one).
4. **remove_dot_segments** (RFC 3986 §5.2.4 algorithm).

Note: path normalization **does not rewrite** `req_header().uri` — upstream / ForwardAuth / mirror receive the raw entry form. Only rewrite plugins legitimately rewrite the URI (via the trait `set_upstream_uri`).

---

## When to use which view (decision guide for plugin authors)

| Scenario | Recommended view | Method |
|----------|------------------|--------|
| Route matching, ACL access control | current | `session.get_path()` / `req.path` |
| access_log default request-path field | current | `$path` / `$uri` |
| Audit: what the client actually sent | entry | `session.get_request_uri()` / `$request_uri` |
| Deny rules that must defend against encoded bypass | current | `RestrictionSource::Path` (auto-normalized) |
| Deny rules that must reject the literal raw form | entry | `RestrictionSource::RequestPath` |
| Record the new path after rewrite | current (already refreshed) | `$uri` |
| Need the original path even after rewrite | entry | `$request_uri` |
| Modify the request path | — | `session.set_upstream_uri(&new_uri)` |
| Default upstream forwarding path view | current normalized | Automatic (v4 spec §14) |
| Per-route force-raw forwarding | entry view | annotation `edgion.io/forward-raw-path: "true"` |
| Per-route force-normalized forwarding (override region_route) | current normalized | annotation `edgion.io/forward-raw-path: "false"` |

**Performance tip**: on the hot path, prefer `path` / `request_path` (`&str`, no extra allocations) over `uri` / `request_uri` (format allocates).

---

## Upstream Path Forwarding decision (v4)

Single-point decision at the entry of `pg_upstream_peer::upstream_peer_http`, with 5 channels by priority:

| Channel | Trigger | Effect |
|---------|---------|--------|
| 0 (implicit, highest) | `ctx.uri_finalized` is already true | Skip the decision (rewrite final state / decision already made) |
| 1 | annotation `edgion.io/forward-raw-path: "true"` | Force raw |
| 1 | annotation `edgion.io/forward-raw-path: "false"` | Force normalized (override region_route) |
| 2 | `external_jump.preserve_raw_path = true` (region_route class) | raw |
| 3 | global `forward_normalized_path = false` | raw |
| Default | None of the above triggers | normalized |

The single `uri_finalized` flag covers rewrite final state + single-point decision settled + retry / mirror / fan-out consistency.

Legitimate `req_header_mut().set_uri()` writes occur in only 2 places (lint Rule 2):
- `session_adapter::set_upstream_uri` (rewrite class).
- `pg_upstream_peer::upstream_peer_http` single-point decision block (v4).
Both must immediately follow with `ctx.uri_finalized = true`.

For details, see spec `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14.
