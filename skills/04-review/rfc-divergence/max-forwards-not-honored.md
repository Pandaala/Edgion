---
name: max-forwards-not-honored
description: Accepted RFC 9110 §7.6.2 divergence — Edgion does not inspect, terminate-on-zero, or decrement the `Max-Forwards` header for TRACE/OPTIONS; it forwards them as ordinary requests, matching Envoy (which also has no built-in Max-Forwards handling). Reject re-reports asking to implement the decrement/terminate logic.
---

# rfc-003 — `Max-Forwards` not honored for TRACE / OPTIONS

**Status:** ⚠️ accepted divergence · 2026-06-19
**RFC:** RFC 9110 §7.6.2 (MUST, intermediary behavior)

## Conclusion

The HTTP data-plane proxy does not read, terminate-on-zero, or decrement the
`Max-Forwards` request header. `TRACE` and `OPTIONS` requests are routed, run
through plugins, and forwarded upstream like any other request (unless the CORS
preflight path happens to terminate an `OPTIONS`, which is unrelated and narrower).

Suggestions to add a `Max-Forwards` check in `pg_request_filter` (terminate with
a local response when the value is `0`, decrement before forwarding when positive,
`400` on invalid) are **not accepted** — we will not implement this.

## RFC clause

> RFC 9110 §7.6.2: an intermediary that receives `Max-Forwards: 0` for `TRACE`
> or `OPTIONS` MUST NOT forward the message; when forwarding with a positive
> value it MUST decrement the field by one before forwarding.

## Why we diverge

1. **Aligned with Envoy.** Envoy has no built-in `Max-Forwards` filter either —
   it treats the header as an opaque field and forwards it unchanged; honoring it
   requires a custom/Lua filter. Edgion is therefore no worse than the Envoy /
   Envoy Gateway baseline we track.
2. **`TRACE`/`OPTIONS` + `Max-Forwards` is a diagnostic niche.** It exists for
   hop-by-hop trace probing; reverse-proxy/gateway deployments rarely rely on it,
   and `TRACE` is commonly disabled outright at gateways for security (XST).
3. **Not a routing/security boundary.** This is an interoperability/diagnostics
   gap, not an auth or tenant-isolation issue. Gateway API conformance does not
   exercise `Max-Forwards`.
4. **Cost vs value.** Adding method-scoped parsing, a local-response terminator
   for `0`, and a decrement-rewrite on the hot path is non-trivial plumbing for a
   P-low diagnostic header almost nobody sends to a gateway.

## Code shape today

- `pg_request_filter.rs` — `request_filter` builds metadata, matches routes,
  optionally handles CORS preflight, runs plugins, appends forwarding headers,
  then returns `Ok(false)` to continue to the upstream pipeline. No
  `Max-Forwards` branch anywhere; no gateway source references the header.
- CORS preflight (`is_preflight`) is `OPTIONS`-only, mode-gated, and does not
  decrement anything — not an RFC 9110 §7.6.2 substitute.

## Re-evaluation triggers

- A real operator/interop incident where a diagnostic client cannot target the
  gateway as the final `TRACE`/`OPTIONS` hop.
- Gateway API conformance starts asserting `Max-Forwards` semantics.
- A decision to natively terminate `TRACE` at the gateway (e.g. return `405`),
  which would naturally cover the `Max-Forwards: 0` case for `TRACE`.

## Reference

- Task: `tasks/review-rfc/rfc-003-max-forwards-not-honored.md`
- Source: `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
