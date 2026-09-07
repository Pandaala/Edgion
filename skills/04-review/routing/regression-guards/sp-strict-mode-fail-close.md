---
name: sp-strict-mode-fail-close
description: Use when reviewing session-persistence strict-mode findings; covers that strict now fail-closes (503) a RESOLVED sticky target that cannot serve and re-pins retries to it, while a map MISS (removed/unknown token) intentionally re-establishes a fresh session rather than 503.
status: fixed
finding_id: route-09
closed: 2026-06-13
knowledge_type: regression-guards
---

# route-09 — SP strict mode never fail-closes — FIXED (with a documented commitment-model boundary)

## Conclusion

`session_persistence.strict = true` now actually fail-closes instead of silently
load-balancing to a different backend:

- **In-attempt**: when the resolved sticky target is unhealthy/ejected/gone-from-
  snapshot, `get_peer_with_sp_fallback` returns **503** (commits the downstream
  response via `end_response_503`, then `Err(InternalError)` — no pingora retry,
  same mechanism as the SSRF arm) instead of re-running plain LB.
- **Cross-attempt**: a strict retry **re-pins to the same sticky target** (via the
  retained `sp_sticky_addr` mirror) rather than migrating via plain LB; if that
  target is now unable to serve, it flows back into the in-attempt 503.

**Deliberate boundary (the "commitment model"):** strict fail-close covers a
*resolved* sticky target (affinity-map HIT). A map **MISS** — the sticky backend
was removed from the cluster, or the token is unknown/expired — **re-establishes a
fresh session** (plain LB + `InjectNew`), exactly as non-strict does. This is NOT
a strict violation:

- the affinity map is rebuilt from the live cell on every endpoint change, so a
  removed backend's token is simply absent — the gateway has no record it ever
  pointed anywhere;
- GEP-1619 explicitly permits re-establishing after a broken session;
- fail-closing a missed token would require retaining recently-removed-address
  history and would wrongly 503 genuinely stale cookies.

## Re-report guidance

- "strict silently migrates to another backend when the sticky target is
  unhealthy / ejected" → **closed by this fix** (in-attempt 503 + cross-attempt
  re-pin). Verify the strict `Err(status)` arm in `get_peer_with_sp_fallback` and
  `sp_repin_on_strict_retry` in `upstream_peer_http` before re-reporting.
- "strict + a *removed* sticky backend re-establishes / migrates on the first
  attempt instead of 503" → **intentional, not a bug.** This is the commitment-
  model boundary above. Do NOT re-file as a strict-mode defect. Reopening would
  require operator demand to fail-close on vanished backends, which needs
  removed-address history (rejected as over-engineering).
- "strict retry returned 504, not 503" when `retry.backoff` is configured →
  **intentional precedence, not a bug.** The route-06 minimum-wait fail-fast runs
  in `upstream_peer` before the step-5 re-pin/strict-503 site; a retry whose
  remaining deadline budget is `<=` the backoff delay fail-fasts with 504 and
  never reaches the strict-503 arm. The two are mutually exclusive within one
  `upstream_peer` call (no double response). The request was out of budget anyway.

## Core Rationale

**Strict semantics chosen (over Envoy-literal and hard-fail-close alternatives):**
"once this request commits to a resolved sticky target, use it or 503; never
silently migrate." The commitment point is `sp_resolve_target`. Cross-attempt
re-pin (rather than immediate hard fail-close on any retry) was chosen so benign
transient retries (stale pooled connection, h2-GOAWAY) retry the same healthy
backend and can succeed — only a now-unhealthy/gone target yields 503.

**Decision logic is a pure, unit-tested function**
`sp_should_repin_on_strict_retry(sticky_consumed, try_cnt, sp_configured, strict,
has_sticky_addr)` fires only on `consumed && try_cnt>=2 && configured && strict &&
has_addr`. It is the **complement on the `strict` axis** of
`sp_should_refresh_on_cross_attempt_retry` (which requires `!strict`); a
brute-force `sp_repin_and_refresh_are_mutually_exclusive` test pins that the two
cross-attempt paths can never both fire.

## Implementation

- `edgion-gateway/src/ctx.rs` — `sp_sticky_addr: Option<SocketAddr>` mirror
  (retained across attempts; `sp_target_addr` is consumed by `take()`).
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` —
  `sp_resolve_target` sets the mirror on a hit; `get_peer_with_sp_fallback`
  strict arm fail-closes 503; `sp_should_repin_on_strict_retry` (pure) +
  `sp_repin_on_strict_retry` (called in `upstream_peer_http` step 5 before the
  peer dispatch).
- Integration: PerTryTimeout suite — C2 (`strict_repin_on_cross_attempt_retry`,
  re-pin: both upstream attempts share one ip:port) and C1
  (`strict_inattempt_fail_close_503`, active-HC-down sticky target → 503).

## Fix Suggestions Not Accepted

- Hard fail-close on any strict retry (503 immediately, no second attempt) —
  rejected: breaks benign stale-connection retries that would transparently
  succeed; more aggressive than Envoy.
- Fail-close on a missed token under strict (retain removed-address history) —
  rejected: over-engineering, and wrongly 503s genuinely stale cookies.

## Re-evaluation Triggers

Reopen only if:

- Operators demonstrate a need to fail-close on a *vanished* (map-miss) sticky
  backend (would add removed-address history as an opt-in, not a default).
- A future `sp_state` writer or LB-health-filter change breaks the
  `take_sp_target_or_select` → `StickyBackendUnhealthy` path the in-attempt 503
  relies on.

## Reference Cases

- Origin finding: `tasks/todo/route-09-sp-strict-mode-never-fail-closes.md`
  (sp-sticky-refresh design review, 2026-06-11), closed 2026-06-13.
- Design: `docs/superpowers/specs/2026-06-13-sp-strict-mode-fail-close-design.md`;
  plan: `docs/superpowers/plans/2026-06-13-sp-strict-mode-fail-close.md`.
- Related: the non-strict cross-attempt token refresh
  (`sp_should_refresh_on_cross_attempt_retry`) from the sp-sticky-refresh change
  (route-04); these are mutually exclusive on the `strict` axis.
