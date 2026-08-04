# Subtasks

## A. Shared resources and CRDs

- [ ] Add canonical HTTP backend classification.
- [ ] Add typed Service/EdgionAIService EBTP targets and field validation.
- [ ] Add AiProxy `backendSelection` without backend membership.
- [ ] Add credential-pool schema and resolved-secret redaction.
- [ ] Remove AI-service timeout and AiProxy service reference.
- [ ] Add quota action schema and bounds.
- [ ] Update CRDs and schema coverage tests.

## B. Controller

- [ ] Validate and resolve EdgionAIService HTTPRoute backendRefs.
- [ ] Add AI-service reverse dependencies and requeues.
- [ ] Add HTTPRoute-to-EdgionPlugins reverse dependencies and requeues.
- [ ] Add EBTP Service/AIService typed attachment indexes.
- [ ] Implement per-target resolution and oldest-valid-wins arbitration.
- [ ] Generalize EBTP ancestor status.
- [ ] Preserve terminal AI refs, namespace, rule name, and UID through delegation.
- [ ] Requeue EBTP ancestors for old/new AI targets on route and parent changes.
- [ ] Resolve credential Secrets partial-good and update status.

## C. Gateway foundations

- [ ] Compile typed route backends and invalid slots.
- [ ] Audit and branch every Service-only backend consumer.
- [ ] Build one typed destination-policy ArcSwap snapshot.
- [ ] Preserve and clean stable policy/telemetry state handles.
- [ ] Capture immutable request-local route, service, and policy snapshots.

## D. AI selection and retry

- [ ] Build immutable canonical `AiAttemptPlan`.
- [ ] Implement invalid-weight conformance gate.
- [ ] Implement Weighted and Priority.
- [ ] Implement LowestLatency and LowestUsage snapshots.
- [ ] Implement CostWeighted fixed-point selection.
- [ ] Implement credential-first bounded retry rounds.
- [ ] Rebuild provider request and BodyStorage on every dispatch.
- [ ] Integrate HTTPRoute retry, source retry budget, and commitment guard.

## E. Credential and resilience runtime

- [ ] Add Redis RPM/TPM reservation and reconciliation.
- [ ] Add credential Retry-After cooldown and rotation.
- [ ] Generalize outlier state and half-open lease.
- [ ] Apply AI-service circuit and connection policy.
- [ ] Implement Throttle and Downgrade.

## F. Response, accounting, and observability

- [ ] Add attempt-aware final response processor.
- [ ] Add bounded per-attempt EdgionHttp access-log fields.
- [ ] Reconcile failed and final attempt usage/cost.
- [ ] Verify default-off token metrics and fixed label domains.
- [ ] Add no hot-path tracing and no sensitive values.

## G. Verification and handoff

- [ ] Run targeted resource, Controller, Gateway, and Redis tests.
- [ ] Run AI integration scenarios and hot reload.
- [ ] Run the AGENTS.md pre-commit suite and additional guards.
- [ ] Complete implementation, review, coverage, execution, and wrap-up records.
- [ ] Stage local commits only; do not push.
