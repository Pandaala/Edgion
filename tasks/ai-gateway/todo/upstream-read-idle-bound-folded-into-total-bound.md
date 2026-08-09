# Upstream per-chunk idle bound is folded into total-duration bounds, so streaming routes lose it entirely

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Status | todo |
| Found while | designing `docs/superpowers/specs/2026-08-04-response-body-pipeline-design.md` (out of scope there — pre-existing and unrelated to that change) |
| Where | `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:1043` |

## The defect

`peer.options.read_timeout` carries **idle** semantics: pingora's
`read_body_ref()` wraps every single body read in it
(`pingora-core/src/protocols/http/v1/client.rs:438`), so the timer effectively
resets on each received chunk. It is the gateway's per-chunk liveness bound on
the upstream.

But its value is assigned from a fold of three bounds, one of which is a
**total** wall-clock bound:

```rust
// pg_upstream_peer.rs:1043
let folded = fold_peer_timeouts(effective_backend_timeout, request_remaining, grpc_remaining);
if let Some((timeout, _)) = folded {
    peer.options.read_timeout = Some(timeout);
    peer.options.write_timeout = Some(timeout);
}
```

`request_remaining` is the route-level `request_timeout` minus elapsed
wall-clock — a total bound. A long-lived stream (SSE, LLM token streaming) must
therefore disable the total bounds to avoid being cut off mid-stream:
`request_timeout` unset/disabled **and** `backendRequestTimeout: "0s"`. At that
point `folded` is `None`, `read_timeout` is never assigned, and the per-chunk
protection is gone along with the total bounds.

The result: exactly the routes that stream — the ones where a wedged upstream is
hardest to notice — are the ones running with no upstream liveness bound at all.

This is the same shape as the Traefik `writeTimeout` problem: a total-duration
bound is unusable for streaming, so operators disable it, and disabling it takes
the idle protection with it.

## Direction (not yet designed)

Separate the idle dimension from the total dimension, so a per-chunk read bound
still applies when every total bound is disabled. No new operator-facing knob is
obviously required — the route-level entry point already exists via
`backendRequestTimeout`; what changes is that its idle meaning stops being
folded away.

Care needed around `ctx.peer_timeout_source`: it is what lets
`error_while_proxy` distinguish a retryable per-try timeout (BackendRequest,
GEP-1731) from a final deadline expiry. Splitting the fold means deciding what
source a fired idle bound reports, and whether it is retryable.

## Verification bar

- A stream idle past the bound is released **and** a stream producing a chunk
  just inside the bound is never released. A bound that fires on a progressing
  stream is worse than no bound.
- A route with all total bounds disabled still gets the idle bound.
- GEP-1731 retry classification is unchanged for the existing per-try timeout
  case.
