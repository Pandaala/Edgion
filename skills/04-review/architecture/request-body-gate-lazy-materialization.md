---
name: request-body-gate-lazy-materialization
description: Use when reviewing whether the generic request-body gate should read all captured bytes before running its inner plugin.
status: fixed
finding_id: BF-03
closed: 2026-07-28
---

# BF-03 — The Generic Request-Body Gate Captures Without Materializing

## Conclusion

`BodyGatedRequestFilter` completes bounded request-body capture through
`PluginSession::ensure_request_body` and checks the returned `u64` length. It
does not call `read_request_body` merely to enforce the gate.

The wrapped plugin remains responsible for calling `read_request_body` if its
own semantics require the complete bytes. Therefore `RequestBodyBuffer` can
leave a large captured body disk-backed, while DSL `req.body`, RequestMirror,
ForwardAuth, Wasm, and other full-body consumers still materialize when they
actually need the complete value.

## Core Rationale

### Capture and full-byte access are different operations

The gate must prove that the body was captured within the gateway physical cap
and enforce the plugin's logical size limit. Both decisions need only the final
length. Reading a disk spill into one contiguous `Bytes` is unrelated to either
decision.

### Full-body APIs retain their explicit cost

This change does not make `req.body` streaming. Its contract is the complete
body, so evaluating it may materialize a disk-backed request. Avoiding that
cost requires a separate bounded streaming DSL API rather than silently
changing `req.body`.

### Storage-aware consumers stay storage-aware

Pingora replay reads the spill incrementally, and HMAC uses
`ensure_request_body` followed by `BodyStorageView::sha256`. The generic gate
must not pre-materialize the body before either path can use those
storage-aware operations.

## Fix Suggestions Not Accepted

- Make `req.body` silently return chunks or a reader — this breaks its existing
  value semantics.
- Remove `read_request_body` from plugins — plugins that forward, transform, or
  inspect arbitrary body bytes still need the complete value.
- Add a second request-body cache for streaming consumers — the shared
  `BodyStorageView` remains the canonical request-scoped storage.
- Apply plugin logical limits as capture limits — capture uses the gateway
  physical cap so one shared body remains usable by later plugins.

## Re-evaluation Triggers

Re-open this decision only if:

- the generic gate itself gains an operation that intrinsically requires body
  bytes;
- `ensure_request_body` no longer guarantees complete bounded capture;
- request-body storage stops exposing an exact final length without
  materialization.

## Reference Cases

- `edgion-gateway/src/plugins/runtime/body_gate.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `tasks/todo/dsl-streaming-request-body-api.md`
- BF-03 review batch, 2026-07-28
