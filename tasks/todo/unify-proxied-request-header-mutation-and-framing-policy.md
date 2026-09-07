# Unify Proxied Request-Header Mutation and Framing Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define one final request-boundary policy for plugin-mutated framing headers so the proxied body and HTTP framing cannot diverge across protocols, retries, or body transformation.

## Requirements

- Inventory Gateway API HeaderModifier, ProxyRewrite, DSL, Wasm, ExtProc, body transformers, and every final session/request-boundary producer.
- Define `Content-Length` and `Transfer-Encoding` ownership for HTTP/1, HTTP/2, gRPC, retries, bodyless requests, streamed bodies, and modified replay bodies.
- Preserve Pingora's canonical standard and dynamic hop-by-hop sanitization as an invariant.
- Choose configuration rejection or final-boundary canonicalization through one shared policy; do not add plugin-local deny-lists.
- Preserve final authority, deferred credential, forwarded-header, request-body capture, backpressure, and retry ordering.
- Use static non-sensitive errors and canonical access-log diagnostics; add no tracing or parallel mutation mechanism.
- Do not modify existing integration-test files.

## Acceptance criteria

- Deterministic Gateway and fork-level tests cover framing ownership for every supported HTTP version and body mode.
- All request-header producers use or are proven compatible with the canonical policy.
- A standalone wire integration issue is added only if local tests cannot prove the final transport behavior.
