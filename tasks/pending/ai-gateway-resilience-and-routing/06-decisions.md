# Decisions and design review

## Agent review coverage

Three independent read-only audits covered:

- HTTPRoute custom-backend support, ReferenceGrant, status, dependency, delegation, and conformance.
- BackendTrafficPolicy target typing, reusable runtime mechanisms, health semantics, and metrics.
- Pingora retry lifecycle, BodyStorage replay, AI attempt preparation, response processors, and
  per-attempt accounting.

## Adopted findings

- Replace AiProxy's service reference with `HTTPRoute.backendRefs` rather than adding a provider or
  upstream list.
- Introduce a canonical typed backend classifier and typed BackendTrafficPolicy target key. Never
  alias EdgionAIService to a core Service or reuse an untyped `namespace/name` key.
- Keep ReferenceGrant decisions in the existing Controller path and preserve invalid weighted slots
  through a first-stage weighted conformance gate instead of silently redistributing their traffic.
- Add a dedicated AI-service reverse-reference manager; do not overload ServiceRefManager.
- Carry delegated terminal AI refs through the existing flattening lifecycle.
- Use the existing route retry fields for attempts, backoff, status codes, connection failures, and
  per-try timeouts. Keep BackendTrafficPolicy retryConstraint as an aggregate gate only, cap the
  route at seven retries/eight attempts, and retain the non-idempotent opt-in for ambiguous replay.
- Put cross-destination `backendSelection` in rule-level AiProxy while sourcing all candidates,
  order, and weights from the matched HTTPRoute. AiProxy contains no backend list.
- Keep destination resilience in BackendTrafficPolicy and reject fields with no valid AI meaning.
- Build every provider attempt from immutable canonical bytes and use the existing request
  BodyStorage replay path.
- Install an attempt-aware response processor that locks only on the final non-retried provider.
- Generalize the existing passive-ejection core with a single half-open probe lease rather than
  creating an AI-only circuit breaker.
- Store credential aliases and target attribution only in bounded access logs/admin snapshots, not
  Prometheus labels.
- Use one request-stable logical model for quota and access-log identity, with attempt-local provider
  model names and full re-admission after an explicit downgrade.
- Treat `(typed AI service, credential alias)` as the attempt target. Reserve RPM/TPM before dispatch,
  reconcile every attempt to its own credential, and apply Retry-After to the credential first.
- Require exactly one rule-level AiProxy for an AI-forwarding rule and repeat Controller validation
  in gateway compilation.
- Define `dispatch_started` as Edgion's conservative accounting boundary and label retry usage/cost
  as estimated unless authoritative provider usage exists.
- Do not modify the Pingora fork; its attempt loop, replay ordering, and final-response boundary are
  sufficient.

## Rejected findings and alternatives

- `AiProxy.routing.upstreams`: duplicates HTTPRoute backendRefs and creates two sources of truth.
- `BackendTrafficPolicy.aiRetry`: duplicates standard HTTPRoute retry and would add another failure
  policy shape.
- AI picker algorithms in BackendTrafficPolicy: the policy attaches per destination, while these
  algorithms compare a route-specific candidate set and depend on logical model/usage/price.
- Reusing BackendTrafficPolicy targetRefs as a route candidate group: targetRefs express attachment,
  not forwarding membership, and must never inject traffic destinations.
- Extending EBTP to target HTTPRoute rules: technically possible, but it mixes route selection with
  backend policy attachment; the existing rule-level EdgionPlugins ExtensionRef already provides the
  required attachment without another policy scope.
- Treating EdgionAIService as an ExternalJump-only plugin target: bypasses route status, weight,
  policy, and dependency semantics.
- Active provider health checks without an explicit endpoint/auth contract: risks billed or
  side-effecting probes and silent credential exposure.
- Credential alias metric labels: configuration cardinality is unbounded across resources.
- A new AI retry/failover counter: existing project policy keeps per-attempt reason and from/to
  attribution in the access log; retry-budget, circuit, and outlier metrics already cover alerts.
- Provider error-body retry codes: require buffering the main response and violate streaming.

## Approved review decisions

The following points are frozen for implementation:

1. `Priority` interprets post-delegation HTTPRoute backendRefs declaration order as strict priority
   only when rule-level AiProxy selects `backendSelection.type: Priority`.
2. AI backendRefs allow only attempt-local `RequestHeaderModifier` and final-response-only
   `ResponseHeaderModifier` in v1. All other filter types are rejected there, and AiProxy runs
   exactly once at rule level.
3. EBTP remains backend-only and accepts one homogeneous target kind per instance. EdgionAIService
   targets permit `connection`, `retryConstraint`, `circuitBreaker`, and `outlierDetection` while
   rejecting backend-inapplicable sections.
4. Credential resolution is partial-good: invalid entries are skipped and reported, while one
   valid credential keeps the service active.
5. Token-quota `Downgrade` is legal only for identity-model limits and must re-admit the explicit
   target model; it cannot bypass an identity-wide exhausted budget.
6. `EdgionAIService.defaults.timeout` is removed in favor of standard HTTPRoute request/backend
   timeouts and BackendTrafficPolicy connection timeout, avoiding three competing timeout sources.
7. Credential TPM is strict reservation-based admission using the request/service maximum-output
   bound. A credential with TPM configured but no resolvable bound is invalid; ambiguous dispatched
   outcomes and crashes retain the full conservative reservation until its Redis window expires.
8. AI routes support at most seven retries/eight total attempts. Higher HTTPRoute values are
   `Accepted=False`, and the gateway enforces the same ceiling defensively.
9. Unknown-model passthrough is limited to stateless `Weighted` and `Priority`; adaptive and
   price-based pickers require a bounded declared model catalog.
10. `tokenQuota.onExceeded` is an explicit tagged action: default `Reject`, bounded one-shot
    `Throttle`, or explicit-model `Downgrade` with identity-model re-admission and a downstream
    disclosure header.
