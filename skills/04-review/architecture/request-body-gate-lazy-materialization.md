---
name: request-body-gate-lazy-materialization
description: Use when reviewing whether the generic request-body gate should read all captured bytes before running its inner plugin.
status: fixed
finding_id: BF-03
closed: 2026-07-28
---

# BF-03 — The Generic Request-Body Gate Captures Without Materializing

## Conclusion

`RequestFilter` defaults to requiring a materialized request body. The generic
gate therefore preserves its `on_read_failure` policy for capture and
storage-read failures before running existing full-body consumers.

A filter may explicitly declare that capture and length are sufficient.
`RequestBodyBuffer` is the canonical capture-only filter: its gate uses
`PluginSession::ensure_request_body` and checks the returned `u64` length
without reading a disk spill into memory. Request-stage DSL is also
capture-only at the gate: opaque `req.body` methods subsequently scan the
shared storage or mutate its logical segments without forcing one contiguous
copy. RequestMirror, ForwardAuth, Wasm, and other full-body consumers retain
their eager materialization and failure-policy behavior.

## Core Rationale

### Capture and full-byte access are different operations

For a capture-only filter, the gate must prove that the body was captured
within the gateway physical cap and enforce the plugin's logical size limit.
Both decisions need only the final length. Reading a disk spill into one
contiguous `Bytes` is unrelated to either decision.

### Failure policy remains fail-close by default

Moving every storage read into an arbitrary inner plugin could let its own
error policy override the body requirement. The trait therefore defaults to
full materialization; capture-only behavior is an explicit opt-out, not an
implicit property inferred from a plugin name. DSL is safe to opt out because
all of its storage, scan-budget, range, and mutation failures become
`BodyAccess`, which bypasses `errorPolicy` and uses the fixed generic
fail-close response.

### Full-body APIs retain their explicit cost

This change makes `req.body` an opaque logical-body value, not a reader exposed
to scripts. Its methods provide storage-transparent operations: predicates,
hashing, definite JSONPath selection, bounded reads, and request-stage
transactional mutations. The gateway owns incremental I/O and scan budgets;
scripts never receive raw file handles or unbounded streams.

### Storage-aware consumers stay storage-aware

Pingora replay reads the spill incrementally, and HMAC uses
`ensure_request_body` followed by `BodyStorageView::sha256`. The generic gate
must not pre-materialize the body before either path can use those
storage-aware operations.

## Fix Suggestions Not Accepted

- Expose chunks, file handles, or an async reader directly to scripts — this
  leaks transport/storage concerns and weakens centralized scan accounting.
- Make every filter capture-only by default — this can move a storage-read
  failure from the body gate into a weaker plugin-specific error policy.
- Remove `read_request_body` from plugins — plugins that forward, transform,
  or inspect arbitrary body bytes still need the complete value.
- Add a second request-body cache for streaming consumers — the shared
  `BodyStorageView` remains the canonical request-scoped storage.
- Apply plugin logical limits as capture limits — capture uses the gateway
  physical cap so one shared body remains usable by later plugins.

## Re-evaluation Triggers

Re-open this decision only if:

- a capture-only filter gains an operation that intrinsically requires body
  bytes;
- `ensure_request_body` no longer guarantees complete bounded capture;
- request-body storage stops exposing an exact final length without
  materialization.

## Reference Cases

- `edgion-gateway/src/plugins/runtime/body_gate.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `tasks/pending/dsl-request-body-value/dsl-request-body-value.md`
- BF-03 review batch, 2026-07-28
