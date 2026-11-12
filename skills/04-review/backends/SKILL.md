---
name: review-backends
description: Review knowledge for backends, grouped by decision type.
---

# backends review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Active health-check transport is independent of BackendTLSPolicy](intentional-design/active-health-check-transport-is-independent.md)
- [backends-F016 — Probe semaphore acquired per probe, not per cycle](intentional-design/probe-semaphore-per-cycle.md)

## Compatibility constraints

- [No upstream max-connection-duration; long-lived H2 connections are never rotated — DESIGN-TRADEOFF (PINGORA-API-BLOCKED)](compatibility-constraints/no-upstream-max-connection-duration-pingora-blocked.md)

## Regression guards

- [Pure discovery geometry and final runtime materialization](regression-guards/discovery-normalized-geometry-ownership.md)
- [infra3-01 — EndpointSlice ready condition semantics (nil ready is ready)](regression-guards/endpoint-slice-ready-condition-semantics.md)

- [Active health-check port validation](regression-guards/active-health-port-validation.md)
- [AllEndpointStatus backend TLS and endpoint identity](regression-guards/all-endpoint-status-tls-identity.md)

- [Retry-budget counters retain coherent epochs and monotonic windows](regression-guards/retry-budget-epoch-coherence.md)

- [lb-06 — Active health-check probes run in parallel within a service — FIXED](regression-guards/active-hc-probes-parallel-within-service.md)
- [Active health task status-marker ownership](regression-guards/active-health-task-status-ownership.md)
- [BackendTLSPolicy partial publication is incremental and atomic](regression-guards/backend-tls-policy-incremental-atomic-publication.md)
- [backends-F008 — build_client_cert_key duplicate PEM-parse block](regression-guards/build-client-cert-key-dedup.md)
- [backends-F007 — build_peer_from_lb_selection inlines TLS-metadata write](regression-guards/build-peer-tls-helper-dedup.md)
- [Circuit-Breaker Integration Ejection State Is Self-Cleaning — FIXED](regression-guards/circuit-breaker-integration-ejection-state-self-cleaning.md)
- [Circuit Breaker — Local-Origin Failure Dimension Split — IMPLEMENTED](regression-guards/circuit-breaker-local-origin-split.md)
- [Circuit Breaker record_failure total_endpoints == 0 Bypass — FIXED](regression-guards/circuit-breaker-record-failure-zero-total-endpoints.md)
- [Circuit Breaker record_success Gauge Leak — FIXED](regression-guards/circuit-breaker-record-success-gauge-leak.md)
- [backends-F017 — ClusterIP backend address handles IPv6 (no manual brackets)](regression-guards/clusterip-ipv6-no-brackets.md)
- [backends-F014 — Consistent-hash query-arg match decodes percent/plus-encoded param names](regression-guards/consistent-hash-arg-percent-encoded-name.md)
- [ctrl-02 — AI target limits preserve existing Service policy behavior — FIXED](regression-guards/ebtp-ai-target-limits-preserve-service-fanout.md)
- [Captured Service policy generation and degrade-threshold selection](regression-guards/ebtp-atomic-service-snapshot.md)
- [backends-F013 — no-port EndpointSlices fail closed instead of defaulting to 8080](regression-guards/ep-slice-no-port-fail-closed.md)
- [EndpointSlice ownership transitions rebuild both Service generations](regression-guards/endpoint-slice-ownership-transitions.md)
- [DNS Answer Cache Identity and Lifecycle (common-02; origin D0001) — FIXED](regression-guards/dns-answer-cache-identity-and-lifecycle.md)
- [Gateway DNS endpoint table lifecycle (ExternalName gateway mode)](regression-guards/dns-endpoint-table-lifecycle.md)
- [runtime-F010 — backend error path reuses the listener's HSTS decision](regression-guards/hsts-on-plain-http-error-responses.md)
- [backends-F001 — IPv6 health-probe URLs are bracketed; gRPC probe no longer panics](regression-guards/ipv6-probe-url-brackets.md)
- [Service section port proofs and DirectEndpoint authorization](regression-guards/named-port-resolve-against-originating-slice.md)
- [backends-F011 — remove_task dead one-line wrapper](regression-guards/remove-task-dead-wrapper.md)
- [net-F001 — resolve_domain hands unbracketed IPv6 literal to lookup_host](regression-guards/resolve-domain-ipv6-literal.md)
- [backends-F010 — set_service_config duplicate ServiceCbState literal](regression-guards/service-cb-state-new-ctor.md)
- [udp-F017 — UDP upstream socket bind ignores address family](regression-guards/session-bind-address-family.md)
- [backends-F004 — `set_service_config` config update is now an atomic `entry()` swap](regression-guards/set-service-config-toctou-race.md)
- [BackendTLSPolicy target and section isolation (backends-F006)](regression-guards/tls-policy-wildcard-cross-service.md)
- [backends-F002 — Concurrent `upsert_task` no longer leaks orphaned health-check tasks](regression-guards/upsert-task-concurrent-handle-leak.md)

## Superseded decisions

- [ExternalName DNS miss coalescing not implemented (superseded by common-02)](superseded/externalname-dns-miss-coalescing-not-implemented.md)
- [BackendTLSPolicyStore rebuild-strategy complexity comment](superseded/backend-tls-policy-rebuild-strategy-comment.md)
- [backends-F009 — preload collect_*_routes five identical bodies](superseded/preload-collect-routes-macro.md)
- [backends-F005 — `warm_cell` startup hydration is now an atomic `hydrate_if_empty`](superseded/warm-cell-toctou-stale-overwrite.md)
