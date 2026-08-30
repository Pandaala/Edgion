---
name: uri-normalization-review
description: Review decision rules for URI normalization — which scenarios are closed, which still need to be reported, the two lint rules and the allowlist.
---

# URI Normalization Review Decision Rules

> Corresponds to spec §9. Authoritative architecture document: [../../01-architecture/02-gateway/uri-normalization/SKILL.md](../../01-architecture/02-gateway/uri-normalization/SKILL.md)

---

## Closed Scenarios (no longer reported)

The following findings have been covered by the path normalization default mechanism and are **no longer valid vulnerabilities** — close directly:

- Path-based ACL using `KeyGet::Path` (`keyMatch` / `keyExist` with `key: {type: path}`) — path normalization is enabled by default (no `enabled` switch); such rules are already compared on the normalized path and cannot be bypassed by `%XX` / `//` / `/../`. `KeyGet::RequestPath` (`key: {type: requestPath}`) is the entry-time raw path and is never normalized.
- HTTPRoute / TLSRoute / GRPCRoute using `Exact` / `PathPrefix` path matching — config side is normalized at load time + request side is normalized; matching is performed in the same normalized space
- HTTPRoute `RegularExpression` matching — request side is already normalized (note: regex patterns themselves are not normalized; the operator is responsible for writing the regex in normalized space, see "Scenarios still to report" item 5)
- path-parameter `..;/`-style bypass — handled by the `strip_path_parameters` algorithm (step A3)
  - **Footnote (v4)**: only closed when `preserve_path_parameters=false` (default). If the operator enables opt-out (`preserve_path_parameters=true`), the commitment degrades; such findings still need to be reported, prompting the operator to use a `keyMatch` / `keyExist` with `key: {type: requestPath}` regex to block `;` patterns or to fix the ACL rule.
- Standard percent-encoding bypass, double-slash bypass, dot-segment bypass — all within commitment scope; covered by the normalization algorithm

---

## Scenarios Still to Report

The following scenarios **still need to be reported**:

1. **DSL uses `req.request_path` for ACL decisions** (instead of `req.path`) — misuse of the entry raw view for security decisions; the bypass window still exists. Suggest changing to `req.path`, or explain why the entry raw is needed (must be clearly commented)

2. **DSL security rules mixing `req.path` and `req.request_path` in the same decision chain without an intent comment** — prompt the reviewer to confirm operator intent; the two views differ in semantics, mixing them may be a logic error

3. **Upstream is sensitive to literal `%2F` and the business depends on raw forwarding** — Edgion path normalization does not rewrite `req_header().uri`; the upstream receives the raw entry form (this is a design commitment, not a vulnerability). This behavior must be made explicit in documentation/comments

4. **`keyMatch` / `keyExist` with `key: {type: path}` written as if targeting raw** (containing `%XX` literals) — prompt the operator: `KeyGet::Path` is compared after normalization; `%XX` literals will not match; should change to `key: {type: requestPath}` (`KeyGet::RequestPath`) or fix to a normalized form

5. **`RegularExpression` path config containing `%XX` / `//` / `/.` / `..;` literals** — request side is already normalized; regex containing the above literals never matches (silently ineffective); the operator must rewrite the regex into its normalized equivalent

6. **Attacks beyond commitment scope** (overlong UTF-8 / double percent-encoding / non-compliant upstream parsing divergence) — if a clear attack scenario is found, prompt the operator to enable `reject_invalid_percent_encoding = true` (expand the commitment scope). This is not a code bug; it is a known design boundary (see spec §1.4)

7. **HTTPRoute annotation `edgion.io/forward-raw-path` value other than "true" / "false"** — illegal config value; gateway treats as None but the operator's actual intent is lost; prompt to correct to a legal value

8. **`forward_normalized_path = false` + business ACL rules mixing `KeyGet::Path` and `KeyGet::RequestPath`** — under global raw mode both views are effectively raw (`Path` still goes through normalization but upstream receives raw); mixing may mask raw-forward side effects

---

## Lint Rules

### Rule 1: Business Code Must Not Read `req_header().uri.path()` Directly

**Violating form**:

```rust
// ❌ Business code reading pingora raw fields directly
let path = session.req_header().uri.path();
let path = ctx.inner.req_header().uri.path();
```

**Required change**:

```rust
// ✅
let path = session.get_path();          // session trait
let path = ctx.path();                  // ctx canonical
```

**Allowlist** (the following locations may read directly without report):

- The ctx path/uri method implementations themselves inside `edgion-gateway/src/ctx.rs`
- The entry-snapshot code block in `pg_request_filter.rs::build_request_metadata` (the `raw_path` / `raw_query` capture feeding the path-normalization entry layer)
- ACME / well-known protocol handling code (`/.well-known/` path dispatch)
- pingora default forwarding logic paths
- The session trait implementations inside `session_adapter.rs` (adapter implementations of `get_path` etc.)

### Rule 2: Business Code Must Not Bare-Write `req_header_mut().set_uri()`

**Violating form**:

```rust
// ❌ Bare-writing in any business code (other than the session_adapter::set_upstream_uri implementation itself)
session.req_header_mut().set_uri(new_uri);
self.inner.req_header_mut().set_uri(parsed);
```

**Consequence**: ctx cache (`normalized_current_path`) becomes inconsistent with pingora uri, causing ACL / access_log / routing decisions to read a stale path.

**Two legal locations starting from v4** (and each must be immediately followed by `ctx.uri_finalized = true`):

1. `edgion-gateway/src/plugins/runtime/session_adapter.rs::set_upstream_uri`
   - Triggered by rewrite plugins (url_rewrite / proxy_rewrite / DSL `req.set_uri` / Gateway API URLRewrite filter)
   - Write order: `set_uri(parsed)` → `refresh_normalized_path(req, cfg)` → `uri_finalized = true`

2. `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs::upstream_peer_http` single-point decision block (newly added in v4)
   - Entry check `!ctx.uri_finalized`
   - Write order (only the `want_raw=false` branch calls set_uri): `set_uri(normalized)` → `uri_finalized = true` (set unconditionally)

**Rule 2.1** (v4): If `set_uri` calls at the two legal locations above are not immediately followed by `uri_finalized = true` → report mandatorily.

**All rewrite plugins** must change paths via the trait method `session.set_upstream_uri()`; bare calls are not allowed.

---

## Commitment Scope Summary

| Attack type | Default coverage | High-security hardening |
|----------|-------------|----------------|
| Percent-encoding bypass (`%61dmin`) | ✅ | — |
| `%2F` / `%5C` encoded slash bypass | ✅ (decoded then merged) | `reject_escaped_slashes = true` (reject requests containing encoded slashes outright) |
| Double slash / dot-segment bypass | ✅ | — |
| path-parameter `..;/` bypass | ✅ | — |
| Overlong UTF-8 (`%c0%ae`) | ❌ outside commitment | `reject_invalid_percent_encoding = true` |
| Double percent-encoding (`%252E`) | ❌ outside commitment | `reject_invalid_percent_encoding = true` |
| Non-compliant upstream parsing divergence | ❌ outside our control | Operator writes ACL rules per upstream behavior |
