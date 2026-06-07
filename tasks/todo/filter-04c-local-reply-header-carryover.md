---
id: filter-04c
title: Response local-reply — header carry-over edge cases (queued-ops sync/async asymmetry, Set-Cookie)
severity: low
origin: 4th deep-review of filter-04 / filter-04b, 2026-06-03
depends_on: filter-04b (shipped)
---

# Response local-reply: header carry-over edge cases

filter-04 / filter-04b ship a sound core mechanism (a response-phase plugin's
`ErrResponse{status, body}` fully replaces the upstream response, body-present and body-less).
A fourth adversarial review confirmed the core is safe and flagged these **peripheral, low-impact**
carry-over inconsistencies, left as follow-ups (rare combinations; fixes are fiddly and risk the
normal forwarding path).

## 1. Queued response-header ops: sync vs async asymmetry (Important-but-rare)

Request-stage plugins can queue response-header ops into `ctx.pending_response_header_ops` (drained
later onto the response). On a **local reply**:

- **Async response plugin** (Phase 7 `run_upstream_response_plugins_async`): no pre-drain, so
  `build_response_local_reply`'s own `drain_pending_response_header_ops` applies the queued ops to
  the fresh reply header — **preserved**.
- **Sync response plugin** (Phase 6 `run_upstream_response_plugins_sync`): the runner pre-drains the
  queue onto the *upstream* header at the top (`plugin_runtime.rs` ~1072, `mem::take` empties the
  queue) **before** the plugin can return `ErrResponse`. By the time `build_response_local_reply`
  drains, the queue is empty, and the upstream header it was applied to is discarded — **lost**.

So a request-phase-queued response header survives a local reply produced by an *async* response
plugin but is dropped by a *sync* one (e.g. DSL, which is sync Phase 6). Spec intent is to apply
queued ops to the reply, so the sync path is the non-conforming one.

Why not fixed inline: the sync pre-drain exists for the normal forwarded-response path; reordering
or snapshotting it risks double-applying or mis-timing ops on the common path. Needs a careful
un-shared fix (e.g. only drain after the no-local-reply decision, or snapshot the queue for the
builder).

## 2. Set-Cookie (session persistence) dropped on a replaced response (Minor)

`pg_upstream_response_filter.rs` `append_header`s the session-persistence `Set-Cookie` directly onto
the upstream header (not via `pending_response_header_ops`). A local reply builds a fresh header and
discards the upstream one, so sticky-session `Set-Cookie` is **lost** for a replaced response. Rare
(sticky route + response plugin replacing that response), Envoy-similar. Decide: document as
expected, or carry sticky cookies onto the reply.

## 3. Cosmetic

- `fail_to_proxy` returns `can_reuse_downstream: true` for the local-reply case, but the post-proxy
  abort path hardcodes `server_reuse = false` and never reads `FailToProxy.can_reuse_downstream`
  here — so the downstream connection is torn down after the (complete, correct) response. Perf-only;
  the spec note "stays reusable" is optimistic. Either accept the close, or find a non-abort path
  that keeps the downstream alive.

## Acceptance

- Decide intended behavior for queued response-header ops + Set-Cookie on a replaced response, make
  sync and async consistent, and add a unit/integration test asserting it.
- Correct or remove the `can_reuse_downstream`/"reusable" claim.
