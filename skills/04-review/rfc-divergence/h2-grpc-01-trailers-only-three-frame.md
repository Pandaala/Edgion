---
name: h2-grpc-01-trailers-only-three-frame
description: Accepted gRPC-over-HTTP/2 spec divergence — error responses emit 3 frames (HEADERS + empty DATA + TRAILERS) instead of spec-required single HEADERS+END_STREAM. Pingora API limitation. Reject re-reports; see tasks/todo/h2-grpc-01-trailers-only-single-frame.md for unblocking conditions.
---

# h2-grpc-01 — gRPC error responses emit 3 frames instead of spec-compliant single HEADERS+END_STREAM

**Status:** ⚠️ accepted divergence · 2026-05-18
**Spec:** gRPC over HTTP/2 § Responses — Trailers-Only (MUST-equivalent)

## Conclusion

Edgion sends gRPC error responses in a three-frame shape rather than the spec-required
single HEADERS+END_STREAM frame. This is a **Pingora API limitation**, not a choice.
All semantic content (HTTP status, grpc-status, grpc-message, content-type) is correct;
the divergence is wire-framing only. All mainstream gRPC clients tolerate it.

Suggestions to fix this by "just passing `end_of_stream=true`" or "bypassing the Pingora
Session abstraction to write the H2 codec directly" are **not accepted** — see "Why we
diverge" below. A tracked future todo exists for when Pingora unblocks this.

## Spec clause

> "Response-Headers & Trailers-Only are each delivered in a single HTTP2 HEADERS frame block."
> "Trailers-Only is permitted for calls that produce an immediate error."
> — gRPC over HTTP/2 spec § Responses

## Current wire shape

```
Frame 1 — HEADERS (END_STREAM=0): :status 200, content-type: application/grpc
Frame 2 — DATA    (END_STREAM=0): [empty, 0 bytes]
Frame 3 — HEADERS (END_STREAM=1): grpc-status: N, grpc-message: "..."
```

## Code shape today

- `src/core/gateway/runtime/server/error_response.rs:53` — `send_grpc_error_three_frame`:
  all public `end_response_*` functions delegate here for gRPC requests (8 call sites:
  400, 421, 404, 501, 503, 500, 502, 503).
- `src/core/gateway/plugins/runtime/session_adapter.rs:179` — `send_grpc_error_three_frame`:
  same pattern for plugin-runtime termination paths.
- `src/core/gateway/plugins/runtime/session_adapter.rs:68` — `send_grpc_error_direct`:
  plugin-facing public API, same three-frame shape.
- `src/core/gateway/routes/http/proxy_http/pg_fail_to_proxy.rs:49` — external call site
  that delegates to `send_grpc_error_three_frame` with an error code derived from Pingora's
  error-type matching (proxy/upstream failure path).
- All three locations carry inline comments documenting the Pingora constraint.

## Why we diverge

1. **Pingora API limitation (double-layer, verified 2026-05-18 against main branch)**
   - Layer 1 (`pingora-proxy::Session::write_response_header`): the `end_of_stream` parameter
     is consumed only by the response-header filter callback; it is **not propagated** to
     `downstream_session.write_response_header()`. The proxy Session API structurally cannot
     signal END_STREAM on the HEADERS frame.
   - Layer 2 (`pingora-core::v2::server`): even at the H2 level, `write_response_header` and
     `write_trailers` are separate calls. There is no API to combine pseudo-headers and
     trailer-style fields (grpc-status, grpc-message) into one HEADERS block. Emitting a
     true Trailers-Only single frame requires H2 codec ownership that Pingora does not expose.
   - No upstream issue or PR in `cloudflare/pingora` addresses this (checked 2026-05-18).

2. **Semantic correctness is unaffected** — HTTP status 200, grpc-status code, grpc-message,
   and content-type are all correct on the wire. All mainstream gRPC clients (grpc-go,
   grpc-java, grpc-python, grpc-c, tonic) correctly parse the three-frame form; the empty
   DATA frame does not cause errors or message loss.

3. **Envoy contrast** — Envoy emits true Trailers-Only via `encode_headers_(..., end_stream=true)`
   in `encodeLocalReply` because it owns its H2 codec layer. Edgion delegates to Pingora and
   cannot replicate this without bypassing the public abstraction boundary.

4. **Bypass cost > benefit** — Writing raw H2 codec around the Pingora Session would replicate
   Pingora's internal H2 stream state machine (a known regression source) for a P2 finding
   with zero observable client-side impact. Not worth the risk.

## Re-evaluation triggers

- Pingora releases an API that (a) propagates `end_of_stream` through the proxy Session to
  the H2 codec, AND (b) allows trailer-style headers to be folded into the initial HEADERS
  frame.
- A cloudflare/pingora upstream issue or PR explicitly targets Trailers-Only single-frame
  gRPC support.
- An operator-reported incident showing a specific gRPC client fails or warns due to the
  three-frame shape.

## Future work

See `tasks/todo/h2-grpc-01-trailers-only-single-frame.md` for the full unblocking
specification and migration steps (which files to touch, what rename to revert, what
integration test to add).

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a2.md → V1` (CONFIRM)
- Closure: [CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md) → h2-grpc-01
- Related naming: naming-h2-grpc-01 (✅ closed 2026-05-21) — renamed the function
  `send_grpc_trailers_only` → `send_grpc_error_three_frame` to signal the workaround at
  call sites; the spec term is reserved for the future single-frame implementation.
- Pingora source (main): `pingora-proxy/src/lib.rs::Session::write_response_header`,
  `pingora-core/src/protocols/http/v2/server.rs::write_response_header` + `write_trailers`
