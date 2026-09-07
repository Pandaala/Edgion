---
name: review-routing
description: Review knowledge for routing, grouped by decision type.
---

# routing review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Local Reply Drops the Sticky-Session Set-Cookie (Accepted as Designed)](intentional-design/local-reply-drops-sticky-set-cookie.md)
- [http1-04 — Method-mismatched route returns 404 (not 405 + Allow)](intentional-design/method-mismatch.md)
- [route-02 — No per-try timeout field — FIXED-VIA-STANDARD-FIELD](intentional-design/per-try-timeout-via-backend-request.md)
- [route-03 — client-disconnect abort in the retry loop: log-only by decision](intentional-design/retry-client-disconnect-log-only.md)
- [Route Architecture Consistency Review](intentional-design/route-architecture-consistency.md)
- [A build-time route predicate prefilter is not justified — WONT-FIX](intentional-design/route-predicate-prefilter-not-justified.md)
- [Session persistence: two-state delegation inheritance + File/Etcd-only strict — DESIGN-TRADEOFF (won't build three-state)](intentional-design/session-persistence-two-state-inheritance-and-k8s-strict-scope.md)

## Regression guards

- [route-10 — 1xx informational responses pollute response-filter state (status, CB, ht) — FIXED](regression-guards/1xx-response-filter-state-pollution.md)
- [Delegated ExtensionRefs remain local to the declaring route](regression-guards/delegated-extensionref-remains-child-local.md)
- [Delegated Child `PathPrefix /` Inherits the Scope Prefix — FIXED](regression-guards/delegation-root-prefix-inherits-scope.md)
- [Denied L7 Named Jumps Fail Closed — FIXED](regression-guards/denied-named-jump-fails-closed.md)
- [GatewayConfigStore Orphaned Secondary Store Removed — FIXED](regression-guards/gateway-config-store-orphaned.md)
- [HTTP `RouteManager` / `get_global_route_manager` Legacy Aliases — REMOVED](regression-guards/http-route-manager-legacy-alias-removed.md)
- [HTTPRouteMatch with an omitted `path` — FIXED at the deserialization layer](regression-guards/http-route-match-omitted-path-default.md)
- [Denied L4 Backend Refs Fail Closed — FIXED](regression-guards/l4-backend-ref-denied-fail-closed.md)
- [L4 Duplex Relay — Centralized in `l4_duplex.rs`](regression-guards/l4-duplex-relay-centralization.md)
- [L4 Rebuild Orchestration — Centralized in `l4_rebuild.rs`](regression-guards/l4-rebuild-helper-clear-pass.md)
- [Lowest-latency tests must distinguish the minimum](regression-guards/lowest-latency-tests-must-distinguish-minimum.md)
- [route-05 — Per-try timeout test coverage gaps — FILLED + ONE ACCEPTED SKIP](regression-guards/per-try-timeout-test-coverage.md)
- [Radix route candidate precedence is ranked at build time — FIXED](regression-guards/radix-candidate-precedence-rank-precomputed.md)
- [`RadixPath::priority_weight` counts Unicode characters, not UTF-8 bytes — FIXED](regression-guards/radix-priority-weight-char-count.md)
- [route-06 — Retry backoff minimum wait can be skipped under a tight deadline — FIXED](regression-guards/retry-backoff-minimum-wait-conformance.md)
- [Reused-connection retry replays non-idempotent bodies — FIXED (method-safety guard)](regression-guards/retry-non-idempotent-method-guard.md)
- [Shared service backend ref predicate](regression-guards/shared-service-backend-ref-predicate.md)
- [route-09 — SP strict mode never fail-closes — FIXED (with a documented commitment-model boundary)](regression-guards/sp-strict-mode-fail-close.md)
- [Status-Retry Outlier Detection Is Per Attempt — FIXED](regression-guards/status-retry-outlier-detection-per-attempt.md)
- [UDPRoute `edgion.io/edgion-stream-plugins` annotation wiring (udp-F012)](regression-guards/udproute-stream-plugins-annotation-wiring.md)
- [Weighted Invalid BackendRefs Retain Their Traffic Share](regression-guards/weighted-invalid-backendrefs-retain-traffic-share.md)

## Review evidence

- [Retry Keeps the Logical Backend Selection — NOT AN ISSUE](review-evidence/retry-keeps-logical-backend-selection.md)

## Superseded decisions

- [Listener Specificity Algorithm Duplicated Across Two Stores — DESIGN-TRADEOFF / SUBSUMED-BY-COMPANION](superseded/listener-specificity-duplicated.md)
