---
name: uri-normalization-review
description: Review decision rules for URI normalization — which scenarios are closed, which still need to be reported, the two lint rules and the allowlist.
knowledge_type: mandatory-design
---

# URI Normalization Review Decision Rules

> Corresponds to spec §9. Authoritative architecture document: [../../01-architecture/02-gateway/uri-normalization/SKILL.md](../../../01-architecture/02-gateway/uri-normalization/SKILL.md)

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

3. **Upstream is sensitive to literal `%2F` and the business depends on raw forwarding** — route matching uses the normalized path, while the forwarding choice is made at upstream-peer finalization. With `edgion.io/forward-raw-path: "true"` (or the global raw-forward setting), the upstream receives the raw entry form; the default writes the normalized path. This choice must be made explicit in documentation/comments

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

**Path-changing locations** (each must set `ctx.uri_finalized = true` after a successful write):

1. `edgion-gateway/src/plugins/runtime/session_adapter.rs::set_upstream_uri`
   - Triggered by rewrite plugins (url_rewrite / proxy_rewrite / DSL `req.set_uri` / Gateway API URLRewrite filter)
   - Write order: preserve the current request-target authority through
     `set_path_only_request_target` → `refresh_normalized_path(req, cfg)` →
     `uri_finalized = true`

2. `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs::upstream_peer_http` single-point decision block (newly added in v4)
   - Entry check `!ctx.uri_finalized`
   - Write order (only the `want_raw=false` branch changes the target):
     `set_path_only_request_target(normalized)` → `uri_finalized = true`
     (set unconditionally)

**Authority-only exception**: `session_adapter.rs::set_upstream_host` may call
`set_uri` to replace an existing URI authority alongside the Host header. This
keeps HTTP/2 `:authority` aligned with an explicit upstream Host rewrite. It
does not change the path or query, so it neither refreshes the normalized path
nor sets `uri_finalized`; the path forwarding decision still runs normally.

**Rule 2.1**: If a path-changing write above is not followed by
`uri_finalized = true` → report mandatorily.

**All rewrite plugins** must change paths via the trait method `session.set_upstream_uri()`; bare calls are not allowed.

Query-only credential stripping uses the session's structured Query removal
operation. It updates the canonical request owner without eagerly updating the
Pingora header or finalizing the path. Current readers use the owner; only a
concrete-header demand projects the changed Query. The common upstream-peer
barrier runs after all request/backend/provider edits and before transcoder
validation and Pingora's outgoing clone. A failed projection fails locally and
must never forward the old credential-bearing target. Error summaries omit the
target so pre-barrier failures cannot expose stale Query bytes.

Projection preserves HTTP/1 absolute-form through raw-target classification and
HTTP/2 scheme/authority through typed URI parts. The corresponding adapter tests
must validate both the logical view and the eventual concrete target.
The same target-preserving helper covers normalized path forwarding and
path-only URI rewrites. HTTP/1 absolute-form stores its scheme and authority
only in Pingora's `raw_path()`, so these writes use Pingora's canonical
`raw_target_authority` classification and `set_raw_path()` to keep the target
form. HTTP/2 keeps scheme and authority in the URI. An explicit absolute
rewrite replaces authority. A path-only rewrite to `*` is rejected before
changing an absolute-form target; concatenating it after the authority would
change the authority itself.

At fork revision `0b0d8bba9609655bfd7db0fe0add6818bc0ee937`, the
`HTTPRoute/Transport` integration suite observes the normalized target,
query, and authority at both HTTP/1 and HTTP/2 origins for all four
downstream/upstream combinations. `EdgionPlugins/PathNormalization/ForwardView`
checks absolute-form default, raw-path, normalized-path, and URLRewrite
consumers against the origin's path and query. The local integration runner
also enables `includeRawUri` and checks the unchanged entry path and query
through `AccessLogClient`; the shared Kubernetes suite runs the origin checks
without that local admin capability.

The six origin-form ForwardView cases likewise require the complete expected
path and preserved query. The selected `test-http` catch-all backend emits
`Path:` and `Query:` separately; path-prefix acceptance can hide an appended
segment or suffix (ARCH-382). `origin_form_assertions_require_complete_path_and_query`
challenges the shared assertion with those wrong outputs, missing paths, and
missing or incorrect queries for all six expectations. These pure checks verify
the assertion, not a fresh live Gateway or Kubernetes forwarding run.

---

## Frozen query cache source

The canonical [URI dual-view contract](../../../01-architecture/02-gateway/uri-normalization/SKILL.md)
requires ordinary decoded single-key reads to use the logical current Query,
just like whole-string current reads. The owner captures entry bytes once;
`ctx.query_param(name)` reuses that document until edited, then selects the current
replacement or retained-pair selection. Routing predicates, plugin extraction,
conditions, DSL, templates and outbound origin fields share its lazy canonical
decoding and last-decoded-name-wins semantics. Explicit `ctx.entry_query_param`
and access-log projections retain entry provenance and masking requirements.
This supersedes the historical ordinary-getter frozen-source contract: removing
a credential must make later ordinary lookups and Webhook templates see it missing.
Route selection is not automatically repeated after plugin edits.

ARCH-004 removed caller-selected cache sources. The later Query owner replaced
the entry-only HashMap and public `entry_raw_query` field. Early reads return
missing without caching that absence before capture. Plain components borrow raw
spans; encoded components retain canonical decoder results once per backing.
LB retains its separate first-match/raw-value policy. Specialized DSL/WAF/strict
protocol parsers retain their own semantics; this owner does not replace them.

AI baseline state cannot escape its Query owner. Restore uses a new checked epoch
and preserves entry identity and reusable caches. A concrete header clone alone
is not a complete baseline once logical Query edits can be pending.

Historical HashMap identity/allocation checks are not guarantees of this new
representation. Verify the current core, adapter, forwarding and AI tests and
measure the active implementation before making performance claims. Re-evaluate
on changes to entry capture, target mutation, snapshot lifetime, or decoder policy.

---

## Canonical query percent-token contract

`edgion-resources/src/uri_utils.rs::url_decode` consumes up to two Unicode
scalars after each percent sign and borrows that exact input prefix, without
owning or copying a temporary token (ARCH-048). Preserve the token's byte-length
`== 2` gate and `u8::from_str_radix(..., 16)`: leading-plus tokens `%+A` and `%+0`
are accepted. Failed tokens keep their consumed scalars literally, including
any plus; only a plus outside a token becomes a space. Successful adjacent
tokens form one UTF-8 byte run. If any byte makes that run invalid UTF-8, the
entire run falls back to bytewise Latin-1 characters. Malformed tokens and
ordinary scalars flush pending bytes before being appended.

Inputs with neither `%` nor `+` return an owned copy directly, avoiding the
scalar-by-scalar append loop and its output growth. The presence scan also runs
for encoded inputs. In that path, after flushing pending bytes and appending an
ordinary scalar, the decoder copies the following ordinary span up to the next
percent or plus marker with push_str. ASCII marker positions are UTF-8 boundaries;
the next marker remains unconsumed for the existing percent/plus branches. Empty
and plain UTF-8 inputs remain byte-identical; this is not a borrowed API or a
second decoding policy. Output growth may differ; no allocation-count or retained
capacity reduction is claimed.

The output String, byte-run buffer and API remain owned as before. Query parsing
skips empty pairs, gives no-equals pairs empty values and lets the last decoded
duplicate key win. Regression assertions live in
`edgion-resources/tests/unit/uri_utils_test.rs`. This local ownership change
preserves canonical single-key decoding, raw whole-query reads, LB's plain-name
fast path and raw values, and auth's decoded-key comparison with raw surviving
pairs. The separately bounded DSL decoder remains excluded. No measured
performance improvement follows from these source and semantic assertions.
Re-evaluate if token consumption, radix parsing or byte-run fallback changes.

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

## Resolution references

- `ARCH-004` — 2026-10-12; the context owns frozen query-cache source selection.
- `ARCH-048` — borrowed percent tokens preserve scalar, radix and byte-run semantics.

- `g8-005` — —.
- `path-norm-F001` — 2026-05-26.
- `path-norm-F002` — 2026-05-26.
- `P090-003` — 2026-09-26; HTTP/1 absolute-form path writes preserve Pingora's raw target authority.
