---
name: 1xx-response-filter-state-pollution
description: Use when reviewing findings that upstream_response_filter records status/ht/circuit-breaker/retry-on-status for HTTP/1.1 1xx informational headers, or that retry.codes preparse accepts non-4xx/5xx codes; covers why the filter now early-returns on is_informational() and why retry-codes are clamped to 400..=599.
status: fixed
finding_id: route-10
closed: 2026-06-13
knowledge_type: regression-guards
---

# route-10 — 1xx informational responses pollute response-filter state (status, CB, ht) — FIXED

The local integration suite `HTTPRoute/Backend/InformationalRetry` exercises the HTTP/1 wire shape that unit tests could not cover: the first upstream attempt flushes `100 Continue` and then exceeds `timeouts.backendRequest`; the configured retry reaches a second 200 attempt. Raw downstream parsing requires exactly one 100 head followed by one intact final 200 response and a closed connection with no trailing bytes. The transport recorder and canonical access log independently require two ordered attempts, first-attempt 504/`ReadTimedout`, and `BackendRequestTimeoutRetry`.

## Conclusion

For HTTP/1.1 upstreams pingora 0.8 delivers each informational (1xx) header as
a separate `HttpTask::Header`, so every 100-Continue / 103-Early-Hints ran
through `upstream_response_filter` and polluted per-response state. The filter
now **early-returns `Ok(())` on `is_informational() && status != 101`** at the very
top, deferring *all* response-phase bookkeeping to the final header task. Separately,
the retry-codes preparse (`http_route_preparse.rs::parse_retry`) now clamps accepted
codes to `400..=599` to match the upstream Gateway API HTTPRouteRetry CRD.

The same interim/final distinction also applies after a `backendRequest`
timeout. Pingora records a relayed informational header in
`response_written()`, so `error_while_proxy` Case 1.5 uses the canonical
`has_final_response_written` helper rather than treating every `Some` value as
terminal. A preceding 100/102/103 therefore leaves timeout retry open; 101 and
ordinary final responses close it.

**101 Switching Protocols is deliberately excluded from the early-return** — it is
terminal, not interim. After a 101 the h1 client stops reading response headers
(`should_read_resp_header()` is false for 101), so there is no final header to defer
to; the upgrade's ht/CB/`ResponseHeaderModifier`/sync-plugin bookkeeping must run on
the 101 itself. The retained `if !is_informational()` guard on the SP-injection block
is therefore live (not dead): it keeps a 101 out of SP injection, preserving the
pre-fix behavior. Do not "simplify" by early-returning all of `is_informational()` —
that drops the upgrade-response bookkeeping (the regression caught in Step 5 review).

Re-reports of the form "1xx pollutes status/CB/ht", "100-Continue counted as a
circuit-breaker success", "retry-on-status fires on a 1xx header", or "retry.codes
accepts 1xx/2xx/3xx codes" are **not accepted** — verify the early-return and the
`400..=599` clamp are still present before re-filing.

## Core Rationale

**1. 1xx is interim, not the final response**

On h1, pingora's client `should_read_resp_header()` returns true again for
`100..=199`, so the filter is re-entered for each interim header. Recording
`ctx.request_info.status`, stamping `upstream.ht` / `upstream.status`, counting a
circuit-breaker success (1xx < 500), or evaluating retry-on-status against a 1xx
all corrupt state that the final header would otherwise own. The
`is_informational() && status != 101` early-return forwards the interim 1xx
unmodified and lets the final header task do status/ht/CB/retry/SP-injection/
pending-header-ops/response-plugins exactly once. h2 upstreams never surface 1xx
here, so they are unaffected.

That shared classification also owns the downstream-commit retry guard. A raw
`response_written().is_none()` check is incorrect because Pingora exposes a
relayed interim H1 header as `Some`; only `has_final_response_written()` keeps
100/102/103 retryable while treating terminal 101 and normal final responses as
committed. Live H1 unit tests pin both sides of this boundary.

**2. The SP-injection `!is_informational()` guard is retained for the 101 case**

The SP-injection block carries its own `if !is_informational()` guard (added by
sp-sticky-refresh-on-retry). It is NOT redundant with the top-level early-return:
interim 100/102/103 early-return before reaching it, so the only 1xx that flows
through is a terminal 101 upgrade — and the guard keeps that 101 out of SP injection,
exactly preserving the original behavior (no sticky cookie consumed/emitted on a 101).
Do not delete this guard as "dead code", and do not widen the early-return to swallow
101 (that drops the upgrade's ht/CB/header-op/plugin bookkeeping — see point 1).

**3. retry-codes range matches the upstream CRD**

The Gateway API HTTPRouteRetry CRD validates `codes` to 400–599. The preparse
previously accepted `100..=599`, so a file-provider config could list a 1xx code
and (pre-fix) make retry-on-status misfire on an h1 informational header. Clamping
preparse to `400..=599` closes that latent misfire and keeps the file provider in
step with the K8s admission validation. Docs (`retry.md` en/zh-CN) were updated
from "100–599" to "400–599" accordingly.

## Fix Suggestions Not Accepted

- "Early-return on all of `is_informational()` (drop the `!= 101` check)" — 101 Switching Protocols is terminal; swallowing it drops the upgrade response's ht/CB/`ResponseHeaderModifier`/sync-plugin bookkeeping permanently (there is no final header after a 101).
- "Delete the SP-injection `!is_informational()` guard as dead code" — it is live; it is the only thing keeping a terminal 101 out of SP injection.
- "Only skip CB recording, keep status/ht for observability" — the access log must show the final status, not 100; partial skipping reintroduces the pollution.
- "Leave retry-codes at 100..=599 and just filter 1xx at runtime" — the preparse is the right rejection point and the runtime already only fires retry on the parsed set; widening the accepted range diverges from the CRD for no benefit.
- "Use raw `response_written().is_some()` to close timeout retry" — an interim
  H1 response is recorded there too; use the canonical final-response helper.

## Re-evaluation Triggers

Re-open only if:

- A future requirement needs the gateway to act on a specific 1xx (e.g. surface 103-Early-Hints link preload to a plugin) — that would be a new, explicitly-scoped 1xx phase, not a regression of this bookkeeping.
- The upstream Gateway API HTTPRouteRetry CRD widens or narrows its codes range away from 400–599.

## Reference Cases

- Origin: sp-sticky-refresh design review (correctness subagent), 2026-06-11; closed 2026-06-13.
- Source: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs` (early-return and canonical final-response helper), `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs` (per-try timeout retry guard and live H1 tests), `edgion-resources/src/resources/http_route_preparse.rs::parse_retry` (`400..=599` clamp).
- Docs: `docs/en/user-guide/http-route/resilience/retry.md`, `docs/zh-CN/user-guide/http-route/resilience/retry.md`.
- Related: the SP-injection 1xx guard from `route-04-sp-sticky-refresh-on-retry` (now subsumed by the early-return).
