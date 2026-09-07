---
name: hsts-on-plain-http-error-responses
description: get_peer's backend-selection-failure error path now reuses the listener's configured ServerHeaderOpts instead of a fresh default that always carries HSTS. Reject re-reports claiming the backend 500/503 error path sends HSTS on plain-HTTP listeners.
knowledge_type: regression-guards
---

# runtime-F010 — backend error path reuses the listener's HSTS decision

**Status:** ✅ fixed · 2026-05-22
**Severity:** Low (BadLogic) — backend-failure 500/503 carried HSTS on plain-HTTP listeners

## Conclusion

`get_peer` (`edgion-gateway/src/backends/mod.rs`) no longer constructs a fresh
`ServerHeaderOpts::default()` for its upstream-selection-failure error responses. It now takes a
`server_header_opts: &ServerHeaderOpts` parameter and the four call sites in
`pg_upstream_peer.rs` (171, 226, 397, 401) all pass `&edgion_http.server_header_opts` — the
listener's configured opts, which already strip HSTS for non-TLS listeners
(`listener_builder.rs:144-150`).

Re-reports of the form "`ServerHeaderOpts::default()` in `backends/mod.rs` sends HSTS on plain
HTTP error responses", "backend 500/503 fallback carries `Strict-Transport-Security` regardless
of TLS", or "`get_peer` error path ignores the listener's HSTS stripping" are **not accepted** —
verify the current `get_peer` takes the `server_header_opts` parameter before re-reporting.

## What was wrong (before the fix)

`ServerHeaderOpts::default()` (`server_header.rs:27-36`) unconditionally upserts
`Strict-Transport-Security: max-age=63072000; includeSubDomains; preload`. The listener builder
removes it for non-TLS listeners (`opts.remove_hsts()` when `!enable_tls`) and stores the result
on `EdgionHttpProxy.server_header_opts`. But `get_peer`'s error arm built a brand-new
`::default()` inline, so a backend-selection failure (service-not-found → 500, no-endpoints /
address error → 503) on a **plain-HTTP** listener returned an error response carrying HSTS — the
lone outlier among all error paths in `pg_upstream_peer.rs`, which already threaded
`&edgion_http.server_header_opts`.

Per RFC 6797 §8.7 a client MUST ignore HSTS received over plain HTTP, so this was **not** a
security issue — only an observable inconsistency and minor internals exposure that confused
operators debugging traffic.

## Why the fix is correct

The fix reuses the listener's already-correct, authoritative opts rather than re-deriving the TLS
state at the call site (the issue's alternative `session.digest()` suggestion). All four
`get_peer` call sites have `edgion_http` in scope, so threading the reference is a mechanical,
no-new-logic change that makes the error path consistent with every sibling error path. TLS
listeners are unaffected (their opts still include HSTS).

## Fix Suggestions Not Accepted

- "Re-derive `is_tls` from `session.digest()` inside `get_peer`" (the issue's alternative) —
  **not accepted**. It would duplicate the listener's TLS decision; threading the
  already-computed `server_header_opts` is simpler and matches the sibling paths.

## Re-evaluation Triggers

Re-open only if:

- A future change makes `get_peer` reachable from a path that does not own a configured
  `ServerHeaderOpts` (then a default would have to be reintroduced — strip HSTS explicitly).

## Reference Cases

- runtime-F010 (review finding, Round B confirmed; fixed 2026-05-22)
- Listener-side HSTS stripping: `listener_builder.rs:144-150` (`remove_hsts()` when `!enable_tls`).
- Sibling error paths already threading the opts: `pg_upstream_peer.rs` lines 142, 149, 247, 263,
  290, 390.
