---
name: ws-05-no-rfc8441-h2-extended-connect
description: Accepted RFC 8441 (optional) divergence — backend appProtocol=ws/wss forces upstream HTTP/1.1; Edgion ships no WS-over-HTTP/2 (extended CONNECT) path because Pingora's WS tunnel is HTTP/1.1-only. Reject re-reports proposing a hot-path downgrade log or a forceHttp1ForWebSocket CRD field.
---

# ws-05 — Backend `appProtocol=ws/wss` forces HTTP/1.1; no RFC 8441 (WS-over-H2)

**Status:** ⚠️ accepted divergence · 2026-05-21
**RFC:** RFC 8441 (MAY — optional HTTP/2 Extended CONNECT extension)

## Conclusion

When an upstream Service is annotated `appProtocol: ws|wss|kubernetes.io/ws|kubernetes.io/wss`,
Edgion pins the upstream peer to HTTP/1.1 (`set_http_version(1, 1)`). It ships no
RFC 8441 (WebSocket-over-HTTP/2 via Extended CONNECT) path, and emits no log when the
HTTP/1.1 choice is made. This is a missing optional capability, not a protocol violation.

Suggestions of the form "emit a `tracing::info!` on the downgrade" or "add a
`forceHttp1ForWebSocket: bool` CRD field to opt out of the forced HTTP/1.1" are
**not accepted** — see "Why we diverge".

## RFC clause

> "This document defines an extended HTTP CONNECT method for HTTP/2 that allows
> WebSocket connections to be bootstrapped on a single HTTP/2 stream." — RFC 8441 §1

RFC 8441 is negotiated by the server advertising `SETTINGS_ENABLE_CONNECT_PROTOCOL`;
supporting it is optional. No MUST/SHOULD is breached.

## Code shape today

- `backends/peer_builder.rs:54` — `BackendAppProtocol::WebSocket` unconditionally calls
  `peer.options.set_http_version(1, 1)`; there is no H2 branch for WS backends.
- `backends/mod.rs:165,225,357` — `apply_backend_app_protocol` runs inside the
  per-request `upstream_peer` path (data-plane hot path).

## Why we diverge

1. **Pingora limitation.** pingora 0.8.0's WebSocket upgrade tunnel works only over
   HTTP/1.1; there is no upstream Extended-CONNECT / WS-over-H2 primitive. The forced
   `set_http_version(1, 1)` exists *because* the upgrade pipeline requires HTTP/1.1.
2. **The proposed escape hatch is a footgun.** A `forceHttp1ForWebSocket=false` field
   cannot deliver WS-over-H2 — Pingora cannot tunnel it — so flipping it would just
   break the WS tunnel. A config knob must not promise a capability the data plane
   lacks.
3. **The downgrade-log suggestion violates the hot-path tracing prohibition.**
   `apply_backend_app_protocol` is per-request; adding `tracing::*` there is barred by
   the data-plane hot-path tracing prohibition (`skills/03-coding/01-log-safety.md`),
   regardless of "one-shot" intent.
4. **Same transparent-tunnel class as the other WS findings.** WS is forwarded
   transparently and protocol decisions sit closest to the endpoints. Same rationale as
   [[ws-01-03-handshake-not-validated]] and [[ws-04-origin-not-validated]].

## Re-evaluation triggers

- Pingora gains an upstream Extended-CONNECT / WS-over-HTTP/2 primitive — then a real
  `forceHttp1ForWebSocket` (default `true`) gate becomes meaningful.
- Gateway API introduces a WebSocket profile that mandates WS-over-H2 negotiation.
- An operator-reported incident where an H2-only WS backend silently fails and the
  backend cannot self-remediate.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a3.md → V5` (CONFIRM, optional-extension qualification)
- Related divergence: [[ws-01-03-handshake-not-validated]] (defers WS-over-H2 to "tracked separately as ws-05"), [[ws-04-origin-not-validated]]
- Hot-path rule: `skills/03-coding/01-log-safety.md`
- Closure: [CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md)
