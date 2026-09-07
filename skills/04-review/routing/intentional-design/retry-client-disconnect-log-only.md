---
name: retry-client-disconnect-log-only
description: The retry loop intentionally does NOT add a downstream-liveness abort for the backoff sleep / connect window; the accepted mitigation is observability-only — the ClientClosed error code in the ordered access-log diagnostics marks delivery failure to a gone client. Pingora h1 already aborts in-flight attempts on downstream EOF within milliseconds (empirically verified), so the blind window is small. Reject re-reports demanding a real-time abort unless they bring new evidence.
knowledge_type: intentional-design
---

# route-03 — client-disconnect abort in the retry loop: log-only by decision

**Status:** ⚠️ accepted tradeoff (log-only) · 2026-06-11
**Origin:** per-try-timeout feature deep review (ops/resilience reviewer), routed via `tasks/todo/route-03-retry-observability.md`

## Conclusion

The TODO at `pg_upstream_peer.rs` (abort the backoff sleep on client
disconnect) stays unimplemented by decision. Empirical probe (2026-06-11,
HTTP/1.1): Pingora's bidirectional pump detects a client disconnect while an
upstream attempt is in flight and aborts within ~60ms — the request ends 499
with `diagnostics: [{"kind":"error","code":"ClientClosed"}]` and a single
canceled attempt. The original
review's "client pays for attempts × backendRequest (240s) of abandoned
upstream work" therefore over-states the exposure: the only window where
retries continue after a disconnect is the backoff sleep + connect phase.

`pg_fail_to_proxy.rs` calls `record_error(EdgionErrorCode::ClientClosed)` when the
proxy-ending error is a downstream failure (code 499, excluding a literal
upstream `HTTPStatus(499)`) or when writing the final error response fails.
Analysis recipe: records with `ClientClosed` and `upstreams.length > 1` are
retry work spent on a departed caller; per-attempt `et` fields quantify it.

## Re-report guidance

A finding that the retry loop "ignores client disconnect" is known, scoped
(backoff/connect window only), and accepted as log-only. Re-open only with
new evidence — e.g. production log analysis (ClientClosed × upstreams
aggregation) showing the remaining window is material, or a Pingora API that
makes downstream liveness checks during backoff cheap and reliable.

## Resolution references

- `route-03 (client disconnect)` — 2026-06-11 (⚠️ accepted tradeoff, log-only).
