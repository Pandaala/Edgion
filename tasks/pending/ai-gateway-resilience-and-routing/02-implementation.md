# Implementation plan

## Delivery strategy

Implement AI-3 in compiler-checked vertical layers. Each layer keeps the workspace buildable and
adds its own targeted tests before the next layer starts.

## Phase 1: shared schemas and canonical identities

1. Extend `HTTPBackendRef` classification with one canonical enum for Service,
   EdgionAIService, route delegation, and unsupported targets.
2. Extend EBTP targets from core Service to homogeneous Service or EdgionAIService targets, with a
   canonical typed parser, one-to-16 bound, duplicate rejection, and field-applicability validation.
3. Add AiProxy `backendSelection` with `Weighted`, `Priority`, `LowestLatency`, `LowestUsage`, and
   `CostWeighted`, without any backend names or weights.
4. Replace EdgionAIService's single Secret with the bounded credential pool, per-credential limits,
   resolved redacted Secret material, and bounded model/alias catalogs. Remove `defaults.timeout`.
5. Remove `AiProxy.aiServiceRef`; add `backendSelection` and the bounded
   `tokenQuota.onExceeded` tagged action.
6. Update both structural CRDs and schema coverage tests.

## Phase 2: Controller references, status, and delegation

1. Teach HTTPRoute validation and ReferenceGrant handling about EdgionAIService backendRefs while
   preserving invalid weighted slots.
2. Add clear-before-add AI-service and EBTP target dependency indexes. Resource events re-enter only
   the owning workqueue.
3. Register rule-level HTTPRoute EdgionPlugins ExtensionRefs clear-before-add and requeue referencing
   HTTPRoutes on plugin apply/delete.
4. Generalize EBTP arbitration and ancestor resolution around typed Service and EdgionAIService
   attachment keys.
5. Resolve EBTP multi-target existence per key while keeping structural validation policy-atomic.
6. Carry terminal rule name, UID, namespace, and AI refs through delegation flattening for dependency
   and adaptive-picker identity.
7. Requeue AI-service EBTPs for the union of old/new route targets on route, delegation, and parentRef
   changes so ancestor status stays current.
8. Resolve all credential Secrets partial-good, report `PartiallyInvalid`, and requeue on Secret
   rotation or removal.

## Phase 3: Gateway typed routes and effective policy snapshot

1. Compile every backendRef once into Service, EdgionAIService, or Invalid without treating AI
   resources as Services in preload, TLS policy, persistence, or internal-jump consumers.
2. Replace the Service-only EBTP runtime key with a typed Service/AI-service ArcSwap effective map.
3. Preserve stable retry-budget, circuit, ejection, half-open, and adaptive telemetry handles across
   equivalent updates; clean residue by typed key and route UID.
4. Capture one route Arc, AiService snapshot, and effective destination-policy snapshot per request.

## Phase 4: AI request plan, selection, and retries

1. Make rule-level AiProxy create a canonical immutable request and `AiAttemptPlan` without setting
   URI, credential, provider headers, or an ExternalJump.
2. Run the invalid-backend conformance gate once without bypassing earlier terminal filters.
3. Implement all five AiProxy-selected algorithms with the frozen integer formulas and bounded
   background snapshots.
4. Implement bounded retry rounds: credential rotation, service exclusion, new-round reset only
   after a dispatched retryable failure, and fixed 503 on an empty scan.
5. Rebuild and overwrite BodyStorage for every dispatched provider attempt; remove all previous
   provider-owned headers before installing the new target.
6. Reuse HTTPRoute retry triggers/backoff/ceiling and charge the failed source destination's retry
   budget. Preserve the non-idempotent replay opt-in and final downstream commitment boundary.

## Phase 5: credentials, resilience, and quota actions

1. Extend the shared Redis limiter with atomic credential RPM/TPM reserve and reconcile operations.
2. Apply Retry-After to the credential, rotate credentials before services, and retain full TPM
   reservations for ambiguous dispatched outcomes.
3. Generalize the passive-ejection state machine and implement one RAII half-open probe lease.
4. Apply service aggregate concurrency and connect timeout from the selected destination EBTP.
5. Implement bounded one-shot Throttle and explicit-model Downgrade with logical-model re-admission
   and downstream disclosure header.

## Phase 6: final response, accounting, and access log

1. Install one attempt-aware response wrapper and bind translation only to the final non-retried
   provider response.
2. Maintain the eight-entry bounded attempt ledger and 8 KiB serialized access-log cap.
3. Reconcile each credential reservation and the request-stable logical-model quota separately.
4. Record authoritative usage when present and explicitly estimated input/cost after
   `dispatch_started` otherwise.
5. Keep optional token metrics default-off and add no retry/failover metric family.

## Verification gates

- Resource and Controller unit tests for every typed target, conflict, dependency, status, and
  delegation transition.
- Gateway unit tests for conformance weighting, all pickers, retries, snapshots, credential state,
  response commitment, accounting, and cardinality.
- Redis integration tests for reserve/reconcile, cancellation, expiry, and failure policy.
- AI integration scenarios for 429/503 cross-provider fallback, SSE disconnect, half-open recovery,
  hot reload, and credential exhaustion.
- Final project checks in AGENTS.md order, plus CRD coverage, gateway no-tracing, English-only, and
  targeted AI integration suites.

## Commit boundaries

1. Shared schema and CRD changes.
2. Controller typed references, status, and delegation.
3. Gateway route/policy foundations.
4. AI selection, retry, credentials, and accounting.
5. Integration tests and documentation.

Every boundary must build and pass its targeted tests. Commits stay local and are never pushed.
