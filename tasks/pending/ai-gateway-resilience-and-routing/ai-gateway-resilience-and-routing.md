# AI Gateway resilience and routing

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Status | in-progress |
| Type | feature |
| Priority | P0 |
| Issue | N/A |

## AI guide summary

Add multi-upstream AI routing without creating a second routing model. `HTTPRoute.backendRefs`
remains the only source of upstream membership and weight, `HTTPRoute.retry` remains the only
per-request retry trigger and attempt policy, `EdgionBackendTrafficPolicy` owns per-destination
resilience, and rule-level `EdgionPlugins/AiProxy` owns AI selection, protocol, metering, quota, and
translation. Cross-provider retries rebuild each attempt from an immutable canonical OpenAI request
and stop permanently once a final downstream response is committed.

## Requirements

- Support weighted, priority, lowest-latency, lowest-usage, and cost-weighted selection across
  `EdgionAIService` backends declared by an `HTTPRoute`.
- Support cross-provider failover for configured HTTP status codes, connection failures, and
  backend-request timeouts without replaying an already translated provider request.
- Reuse route retry, backend retry-budget, timeout, outlier, and circuit-breaker mechanisms.
- Add a credential pool within each `EdgionAIService`; exhaust credentials before moving to a
  different route backend.
- Account for every upstream attempt in access logs, token quota, and estimated cost.
- Preserve streaming and body-memory invariants; never fail over after final response commitment.

## Scope

### In scope

- `EdgionAIService` as an implementation-specific HTTPRoute backend.
- Same-namespace and ReferenceGrant-authorized cross-namespace references.
- Typed backend-policy target identity for Service and EdgionAIService.
- AI attempt planning, provider translation, credential selection, retry arbitration, telemetry,
  and bounded access-log attribution.
- Shared single-probe half-open behavior for passive ejection.

### Out of scope

- A second upstream list inside AiProxy.
- Provider-error-body inspection or buffering for retry decisions.
- Active health probes for public AI providers in the first implementation.
- Semantic prompt routing, multimodal capability downgrades, and response buffering.
- Prometheus labels containing model, AI service, credential alias, endpoint, or tenant identity.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `00-context.md` | done | Skills and code entry points |
| `01-design.md` | done | Approved architecture and schema |
| `02-implementation.md` | done | Phased implementation and verification plan |
| `03-subtasks.md` | active | Ordered implementation checklist |
| `06-decisions.md` | done | Agent findings and approved decisions |

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-resources` | Backend classification, AiProxy and AI-service schema, typed policy targets |
| `edgion-controller` | Route dependency/status, AI-service refs, policy arbitration and requeue |
| `edgion-gateway` | Route candidate selection, AI attempts, peer construction, retry and telemetry |
| `config/crd` | EdgionAIService and EdgionBackendTrafficPolicy schema changes |
| `edgion-tests` | Controller, runtime, Redis, failover, streaming, and hot-reload coverage |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-04 | Use HTTPRoute backendRefs as the sole upstream set | Avoid duplicate routing truth in AiProxy |
| 2026-08-04 | Use HTTPRoute retry and BackendTrafficPolicy retryConstraint | Avoid a fifth retry policy shape |
| 2026-08-04 | Configure `backendSelection` in rule-level AiProxy | Attach through the existing HTTPRoute ExtensionRef without making EBTP multi-scope |
| 2026-08-04 | Keep EBTP backend-only | Align with Gateway API backend-policy semantics and avoid route/backend attachment ambiguity |
| 2026-08-04 | Do not modify the Pingora fork | Existing attempt replay and final-response guards provide the required seams |
| 2026-08-04 | Run a weighted invalid-ref gate before every AI picker | Preserve Gateway API proportional 500 behavior |
| 2026-08-04 | Cap AI execution at seven retries/eight attempts | Bound replay, accounting, and access-log memory |
| 2026-08-04 | Keep retry transitions in access logs, not a new counter | Follow the existing retry observability decision and avoid metric growth |
