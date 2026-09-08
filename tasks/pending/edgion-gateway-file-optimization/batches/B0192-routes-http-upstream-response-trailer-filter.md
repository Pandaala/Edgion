# B0192 — HTTP Upstream Response Trailer Filter

## Scope

- Analysis baseline: `12ec874aa1475eb6f984e77ad19cbad3cc3e319a`
- Target blob: `d246e25da366fb6bb0ec79cbcb936d506d9d4199`
- Pipeline state: ready to commit
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_trailer_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. Three gateway/plugin-owned trailer failure exits use `InternalError`, so `pg_error_while_proxy` can record a selected backend `LocalOrigin` failure after its response head already settled the backend outcome.
- Frozen technical scope: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_trailer_filter.rs`, including inline tests only.
- Required behavior: use one static explicitly non-retryable `HTTPStatus(500)` for the unfrozen callback, unavailable cancelled lease, and companion fail-close paths; preserve existing bounded plugin/access-log outcomes and add no diagnostic, metric, trace, or dynamic error detail.
- Required preservation: real-versus-absent trailers, exactly-once terminal claims, serial mutation-visible companion order, fail-close preemption, cancellation release, logging fallback, `TerminalBeforeTrailers`, body streaming, RawWindow, AI/SSE, cache, and retry closure remain unchanged.
- Focused test baseline is pending because the concurrent B0190 draft temporarily prevented compilation; run it after current-HEAD freshness revalidation.
- This queued candidate cannot enter the serial mutation/review lane before B0190 and B0191 complete.
- Previous batch: `5d5379b0`; never push.
- Freshness: revalidated at `5d5379b076983e7c14d7c90a08522ed39301064f`; target blob remains `d246e25da366fb6bb0ec79cbcb936d506d9d4199`. The remote observability decision concerns an optional response-header debug surface and reinforces, rather than changes, this batch's no-new-observability constraint.
- Focused baseline: 9/9 tests passed.

## Proposed Implementation Plan

1. Add one private static, non-sensitive, explicitly non-retryable HTTP 500 helper.
2. Use it at exactly the unfrozen-callback, unavailable-lease, and companion-fail-close exits without moving lifecycle operations.
3. Extend inline tests for exact type/retry behavior and deterministic lease cancellation while retaining existing trailer lifecycle coverage.
4. Keep logging, diagnostics, metrics, tracing, buffering, integrations, and every adjacent response lifecycle unchanged.
5. Verify the target, full proxy-http module, exact single-file technical scope, repository gates, and frozen integrations.

Independent plan review approved the plan without a Must Fix.

## Outcome

- Implementation: all three gateway/plugin-owned trailer failures now use one static, explicitly non-retryable HTTP 500, preventing selected-backend local-origin attribution without moving claims, serial companion execution, log finalization, lease completion, or return points.
- Review: fresh independent final review returned LGTM with no Must Fix or Should Fix; exact type/retry assertions cover unfrozen, closed-lease, and companion fail-close paths.
- Verification: target 10/10 and full proxy-http 265/265; `cargo fmt --all`, locked workspace check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration-path checks passed.
- Commit: pending; never push.
