---
name: uri-normalization
description: Authoritative document for URI normalization architecture — full-stack naming table, dual-view concept, iron rules, security guarantee scope, set_upstream_uri integration point.
---

# URI normalization (Path Normalization)

> Authoritative source: `docs/superpowers/specs/2026-04-26-uri-normalization-design.md`
> Related review rules: [../../04-review/uri-normalization/mandatory-design/normalization-contract.md](../../../04-review/uri-normalization/mandatory-design/normalization-contract.md)

Edgion enforces path normalization at every request entry (cannot be disabled), eliminating gaps where path-class ACLs could be bypassed via percent-encoding / double slashes / dot-segments / path-parameters.

---

## Concept: dual-view architecture

| View | Reflects | Affected by rewrite plugins | Typical use |
|------|----------|------------------------------|-------------|
| **current view** (default name) | post-rewrite current path/uri, normalized | Yes (the adapter automatically refreshes inside `set_upstream_uri`) | Route matching, ACL decisions, default access_log |
| **entry view** (`request_` prefix) | raw form at the client entry, **frozen** | No | Audit, logging, special raw-deny scenarios |

Method identity follows the same entry/current distinction without changing URI
ownership: `session.get_method()` borrows the current rewritten method, while
`session.get_request_method()` borrows `Option<&str>` from the canonical entry
Method in `RequestInfo`. Missing entry metadata is explicit and never falls back
to the current method. `${method}` and `KeyGet::Method` remain current; omitted
outbound method uses the entry view. Method capture follows early RequestId-only
handling and Pingora request modules, before routing and ordinary rewrite plugins.
The [plugin identity contract](../12-edgion-plugin-dev.md#entry-and-current-request-identity)
describes HEAD/CONNECT admission and caller-specific missing policies.

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
                      ctx.query()                    (current view, whole-string passthrough)
                      ctx.entry_query_param(name)    (entry view, single-key decoded, init-once)
                      ctx.uri()                      (current path + current query)
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
| | `ctx.uri() -> String` | `ctx.request_uri() -> String` |
| | `ctx.query() -> Option<&str>` (whole, raw, passthrough) | `ctx.entry_query_param(name) -> Option<&str>` (single-key, decoded, frozen) |
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
| **`request_restriction` condition** | `keyMatch` + `key: { type: path }` | `keyMatch` + `key: { type: requestPath }` |
| | (whole-query key not supported) | `keyMatch` + `key: { type: query, name }` (single-key, decoded, frozen) |

### Naming rules

- Default name (`path` / `uri`) = the current normalized view (cached, reflects rewrite).
- `request_` prefix = entry raw view (frozen, unaffected by rewrite).
- query is not normalized; whole-query access uses a single name (`query`).

### Query ownership and views

`EdgionHttpContext::request_query` owns the frozen entry raw query and the logical
current query. `ctx.entry_query()` borrows entry bytes; `ctx.entry_query_param(name)`
returns the last decoded-name match and decoded value. Routing, ordinary plugin
single-key reads and access-log query projections use that frozen entry view.
`ctx.query()` returns the current raw query; `ctx.uri()` combines it with the
current normalized path. Current readers never fall back to Pingora's possibly
stale query after entry capture.

Capture moves the existing raw String into the owner without parsing. Tokens,
decoded names/values and the optional lookup index initialize only on demand;
plain components borrow raw spans. Encoded components use the canonical resource
URI decoder. Reads before capture return missing without poisoning later capture.
Raw syntax, duplicates, bare flags and absent versus empty query stay represented.
Canonical full-URI composition preserves a trailing `?` for an explicitly empty
Query; absence has no delimiter. This applies to forwarding, signatures and mirror
views as well as raw accessors. The empty-string setter still means removal.
Structured removal preserves surviving raw bytes and order; no-match removal
preserves every original byte. A changed selection renders once when raw bytes
are demanded. Replacements create a new backing; successive deletions reuse the
existing backing and decoded cells without retaining a history of edits.

LB has its own policy over the same owner: first pair containing `=`, raw-name OR
decoded-name equality, and borrowed raw value. The first empty matching value
terminates lookup. Auth removes all decoded-name matches, including bare flags.
These policies must not be replaced by frozen last-wins entry lookup.

Query-only edits remain logical and do not finalize the path or update Pingora
immediately. Concrete target consumers project the current query when needed.
The common upstream-peer callback synchronizes after backend/provider/authority
writes and before concrete transcoder validation, before Pingora clones and
sanitizes the outgoing header. Request-stage transcoder validation has its own
conditional barrier. `uri_finalized` controls the path policy, not query sync.
Cache keys, mirror snapshots, signatures and plugin reads consume the owner.
A failed final projection fails locally without retry or sending a stale target.

AI keeps its concrete header baseline paired with a private baseline inside the
same Query owner. Restore advances the checked projection epoch; no transferable
entry-alias snapshot exists. Detached exchanges export an owned target. Error
summaries omit the request target, so a stale concrete query cannot expose removed
credentials on a failure before synchronization.

This representation trades first-use token storage and lookup overhead for shared
parsing and deferred assembly. It does not promise a universal CPU/allocation win;
see the implementation verification record for measured limits.

### query view asymmetry (whole-string only — partially closed)

`$request_query` / `req.request_query_string` (a whole-string entry view) still does not exist. The entry whole query is only available indirectly via `$request_uri` — split it from there. ROI of adding such a variable is low (most audits care about the full URI). YAGNI.

Single-key entry-view access **does** exist (`get_query_param(name)` / `req.query("name")`); it has been entry-frozen and decoded since the unified-query-string-cache landing (2026-05-10).

---

## Iron rules and allowlist

**Iron rule**: business code MUST NOT read `req_header().uri.path()` directly; it MUST use `ctx.path()` or `session.get_path()`.

Allowlist (direct reads at the following sites are legitimate):

- The ctx path/uri method implementations themselves in `edgion-gateway/src/ctx.rs`.
- The entry-snapshot block in `pg_request_filter.rs::build_request_metadata` (the `raw_path` / `raw_query` capture feeding the path-normalization entry layer).
- ACME / well-known protocol handling code.
- Pingora default forwarding logic paths.
- The session trait implementations in `session_adapter.rs`.

---

## Canonical target writers

Rewrite plugins use `session.set_upstream_uri()`. The adapter validates the native
target and normalized path before publishing the header, Query owner and path
state together. An explicit whole-target rewrite replaces current Query but never
entry Query. `set_upstream_host()` preserves the logical query and dirty state.
Query-only setters validate input but defer header publication; structured auth
removal does not render a replacement. Both retain sealed-transcoder protection.

Native target projection uses the canonical target-form helper: HTTP/1
absolute-form authority lives in Pingora raw bytes, while HTTP/2 scheme/authority
live in the typed URI. Preserve both forms. Path finalization retains the selected
raw/normalized policy and uses the owner query. Snapshot projection does not mark
the main header synchronized. The common pre-send barrier remains necessary even
when the path is already finalized.

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

Path normalization updates the current path view at entry. ForwardAuth and mirror use
the normalized current path with logical current Query; main upstream forwarding
applies the raw/normalized policy below at its existing finalization stage.

---

## When to use which view (decision guide for plugin authors)

| Scenario | Recommended view | Method |
|----------|------------------|--------|
| Route matching, ACL access control | current | `session.get_path()` / `req.path` |
| access_log default request-path field | current | `$path` / `$uri` |
| Audit: what the client actually sent | entry | `session.get_request_uri()` / `$request_uri` |
| Deny rules that must defend against encoded bypass | current | `keyMatch` + `key: { type: path }` (values auto-normalized) |
| Deny rules that must reject the literal raw form | entry | `keyMatch` + `key: { type: requestPath }` |
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

`uri_finalized` records the selected path policy across rewrites and retries.
Query consistency comes from the owner and its projection barrier; a finalized
path does not imply the concrete Query is synchronized.

Path-changing writes set `uri_finalized`; authority-only writes and Query-only
projection do not select a path policy. Native header writes are confined to the
canonical adapter/target helpers and forwarding barrier. AI baseline restoration
must restore its paired owner state and invalidate the destination projection.

For details, see spec `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14.
