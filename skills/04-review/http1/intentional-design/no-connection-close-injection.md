---
name: no-connection-close-injection
description: RFC 9110 §7.6.1 — Edgion does not inject `Connection: close` into proxied upstream responses; downstream wire framing is owned by Pingora codec via set_keepalive(None). Reject re-reports asking for manual injection during graceful shutdown.
knowledge_type: intentional-design
---

# http1-02 — No manual `Connection: close` injection in proxied responses

**Status:** ✅ fixed · 2026-05-19
**RFC:** RFC 9110 §7.6.1 (MUST)

## Conclusion

Edgion does NOT inject `Connection: close` into `upstream_response` in
`pg_response_filter`. During graceful shutdown, the downstream session's
`set_keepalive(None)` (called from `pg_early_request_filter`) causes Pingora's
H1 server codec to emit `Connection: close` on the wire automatically.

Suggestions of the form "add `upstream_response.insert_header("Connection", "close")`
during shutdown to signal LB drain" are **not accepted**.

## RFC clause

> "A proxy or gateway MUST remove any connection options from a forwarded
> message, since those header fields apply to the immediate connection, not
> to subsequent recipients of the message." — RFC 9110 §7.6.1

## Code shape today

- `pg_early_request_filter.rs::early_request_filter` calls
  `session.set_keepalive(None)` during process shutdown; this is the **only**
  mechanism by which downstream connection close is signaled during shutdown.
- `pg_response_filter.rs::sanitize_forwarded_response_headers` removes received
  static and dynamically nominated hop-by-hop fields before an ordinary
  proxied response reaches the downstream writer. It runs before and after
  async response plugins; 101 upgrade responses retain their handshake fields.
- Wire emission of `Connection: close` is handled by the pinned Pingora fork's
  `pingora-core/src/protocols/http/v1/server.rs`: when response-header updating
  is enabled, the codec sets `Connection` from `will_keepalive()`. H2 removes
  connection-specific fields instead of emitting an H1 connection directive.

## Why this shape

1. **RFC 9110 §7.6.1 MUST + Pingora already covers wire emission.** Manual injection in the proxy `response_filter` hook is both redundant (Pingora rewrites the header at codec layer) and a MUST violation (proxy-forwarded message contains a hop-by-hop directive).
2. **No operator-visible regression.** `set_keepalive(None)` still drains the TCP connection after the response; downstream LB / CDN sees both the codec-emitted `Connection: close` header and the actual TCP FIN.
3. **No `tracing::*` on the data-plane hot path.** Removing the injection also removes one redundant `insert_header` allocation per shutdown-window response.

## Test coverage (intentional gap)

No targeted integration test was added because:

1. The fix changes **no wire-observable bytes** — Pingora's H1 codec rewrites `Connection` based on `will_keepalive()` regardless of what the filter chain leaves. Pre-fix wire and post-fix wire are identical for shutdown-window responses.
2. The only meaningful regression target ("graceful-shutdown responses still carry `Connection: close`") would be asserting a Pingora codec invariant, not Edgion code; that belongs in a Pingora-upgrade smoke test, not in this fix's regression suite.
3. Triggering graceful shutdown mid-request from an integration test (SIGTERM + in-flight request observation) has no template in `edgion-tests/`; building one from scratch is disproportionate to the value.

If Pingora is later upgraded and the `update_resp_headers` default flips or its
H1 connection rewrite is removed, re-evaluate downstream framing ownership as
an explicit Pingora-integration change. Do not restore an upstream
`Connection` field or bypass the response sanitizer as an implicit fallback.

## Re-evaluation triggers

- Pingora stops auto-injecting `Connection` based on `will_keepalive()` (would surface as a regression in wire framing during graceful shutdown).
- A new transport (HTTP/3) is added where keepalive signaling needs explicit handling at the proxy filter layer.
- An operator-reported incident shows the downstream LB / CDN ignoring the codec-emitted header (highly unlikely; the header byte stream is identical).

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a1.md → V4` (CONFIRM)
- Pingora codec: pinned Edgion fork
  `pingora-core/src/protocols/http/v1/server.rs::prepare_response_header_for_write`
- Source:
  `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs::sanitize_forwarded_response_headers`,
  `pg_early_request_filter.rs`

## Resolution references

- `http1-02` — 2026-05-19.
