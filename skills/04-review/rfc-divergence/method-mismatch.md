---
name: method-mismatch-returns-404
description: Accepted RFC 9110 §15.5.6 divergence — method-mismatched route returns 404 (not 405 + Allow), because Gateway API conformance hard-asserts 404. Reject re-reports of the same suggestion.
---

# http1-04 — Method-mismatched route returns 404 (not 405 + Allow)

**Status:** ⚠️ accepted divergence · 2026-05-17
**RFC:** RFC 9110 §15.5.6 (MUST)

## Conclusion

When an HTTPRoute / GRPCRoute path matches but the method filter rejects, Edgion returns `404` (no `Allow` header), not `405 Method Not Allowed`.

Suggestions to add a `MatchOutcome::PathMatchedMethodRejected` variant, accumulate `allowed` across candidates, and introduce `end_response_405` are **not accepted**.

## RFC clause

> "A server generating a 405 response MUST generate an Allow header field
> containing a list of the target resource's currently supported methods."

## Code shape today

- `edgion-gateway/src/routes/http/match_unit.rs::HttpRouteMatchUnit::matches_common`
  — method mismatch returns `Ok(None)`; no "rejected by method" signal.
- `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs`
  — after all candidates fail, collapses to `Err(RouteNotFound)`.
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
  — maps `RouteNotFound` to `end_response_404`; there is no 405 path.

## Why we diverge

1. **Gateway API conformance hard-asserts 404.** `HTTPRouteMethodMatching` (`sigs.k8s.io/gateway-api/conformance@v1.5.1/tests/httproute-method-matching.go` lines 60-62, 101-110; identical in v1.4.0) expects `StatusCode: 404` for `HEAD /`, `PUT /`, `DELETE /path4` against routes declared with other methods. Conformance is currently ALL PASS; 405 would regress it.
2. **Aligned with Envoy Gateway / Istio / nginx-gateway** — all return 404 here for the same reason.
3. **Plumbing cost** is non-trivial: cross-candidate `Allow` aggregation that the radix engine intentionally discards, plus new status / helper / gRPC mapping — for a P2 we'd lose to conformance anyway.

## Re-evaluation triggers

- Gateway API updates `HTTPRouteMethodMatching` to expect 405 + Allow.
- Operator-reported incident shows real harm (auto-discovery tooling broken with no workaround).
- Edgion drops `SupportHTTPRouteMethodMatching`.

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a1.md → V6` (CONFIRM)
- Conformance: `skills/08-kubernetes/01-gateway-api/SKILL.md` (ALL PASS, must remain so)
