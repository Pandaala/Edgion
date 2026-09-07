---
name: wasm-local-response-duplicate-headers
description: Use when reviewing WASM plugin findings that proxy_send_local_response / finish_local_response collapses repeated response header names (e.g. multiple Set-Cookie) to the last value; covers the set-then-append fix and why per-name dedup is not done.
status: fixed
finding_id: runtime-04
closed: 2026-07-18
knowledge_type: regression-guards
---

# WASM local responses collapse duplicate response headers — FIXED

## Conclusion

`finish_local_response` (`edgion-gateway/src/plugins/http/wasm/plugin.rs`) applied
**every** header pair from the guest's `LocalResponse` with `set_response_header`
(→ `insert_header` / `PendingHeaderOp::Set`, both replace-all), so repeating a
name — the common case being two `Set-Cookie` values — kept only the last value.
Fixed by choosing set semantics for the **first** occurrence of each name
(case-insensitive) and append semantics for later ones, mirroring the already-fixed
CSRF/OIDC/DSL writers (gateway-plugins-02) and the request-side F-001.

Re-reports of the "WASM local response drops duplicate Set-Cookie / collapses to
last value" form are **not accepted** — verify `finish_local_response` still does
set-first-then-append before re-filing.

## Core Rationale

**1. The ABI preserves the repeats; the writer destroyed them**

`proxy_send_local_response` (`host_abi.rs`) decodes the guest's serialized header
map into a `Vec<(String, String)>` that legitimately contains repeated names. The
defect was purely in the terminal writer, which looped `set_response_header` over
every pair. Both stage paths of `set_response_header` are replace-all: request
stage queues `PendingHeaderOp::Set` (drained via `insert_header`), response stage
calls `insert_header` immediately. `Set-Cookie` is a RFC 6265 §3 / RFC 7230 §3.2.2
list-syntax exception that must NOT be comma-coalesced, so the correct primitive is
`append_response_header` (→ `PendingHeaderOp::Append` → pingora `append_header`).

**2. Set-then-append, not append-for-all**

Using append for *every* pair would preserve a carried-over same-name value from an
earlier plugin instead of replacing it — a local response is a full short-circuit
reply and its first write of a name should own that name. So the fix sets on first
sight of a name and appends thereafter, which both replaces stale carry-over and
keeps every distinct guest value.

**3. No per-name value dedup**

Same as gateway-plugins-02: the queue drains after any upstream header and the
browser applies same-name `Set-Cookie` last-wins (RFC 6265 §5.4), so name-level
dedup is intentionally NOT performed.

**Scope note — request stage only.** The fix is effective on the request-stage
local reply (the realistic auth-guest 302/redirect path), where the writes are
queued as `PendingHeaderOp`s and later drained onto the fresh reply header. At the
**response** stage a bound `ResponseHeader` already exists, so `set/append` mutate
the *upstream* header directly and queue nothing; when the guest replaces the
response, `build_response_local_reply` builds a **fresh** header and only re-applies
the (empty) pending queue, so ALL of the guest's local-response headers are dropped
there — duplicate-collapse is moot because nothing survives. That discard is the
separate, intentional `filter-04c` design (replaced responses drop directly-set
upstream headers), NOT a runtime-04 regression; do not re-file it here.

## Fix Suggestions Not Accepted

- Reroute `Set-Cookie` to append inside `PendingHeaderOp::Set::apply` — masks the
  call-site bug at the drain layer instead of fixing the origin writer (same
  rejection as gateway-plugins-02).
- Append-for-all pairs — would leak a stale carried-over value for the first
  occurrence of a name.
- Special-casing only `Set-Cookie` by name — the set-then-append rule is correct
  for every repeatable field (`Vary`, `WWW-Authenticate`, `Link`, …), not just
  cookies.

## Re-evaluation Triggers

Re-open only if:

- `finish_local_response` is refactored to bypass `PluginSession` and write the
  `ResponseHeader` directly (the drain-path assumption changes), or
- A future ABI lets the guest express explicit set-vs-append intent per header, in
  which case the host must honor that instead of inferring first-vs-later.

## Reference Cases

- Finding `runtime-04` (2026-07-18), `tasks/review-wasm-issues/`.
- Fix + regression test: `finish_local_response` and
  `plugins::http::wasm::plugin::tests::local_response_preserves_duplicate_set_cookie`
  (drains queued ops onto a real `ResponseHeader`, asserts both cookies survive).
- Response-side sibling: [../csrf/set-cookie-append.md](../../csrf/regression-guards/set-cookie-append.md)
  (gateway-plugins-02). Request-side sibling:
  [append-request-header-multi-value.md](append-request-header-multi-value.md) (F-001).

## Resolution references

- `runtime-04` — 2026-07-18.
