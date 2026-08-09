# Subtasks

## A. Shared resources and CRDs

- [x] Add canonical HTTP backend classification.
- [x] Add typed Service/EdgionAIService EBTP targets and field validation.
- [x] Add AiProxy `backendSelection` without backend membership.
- [x] Add credential-pool schema and resolved-secret redaction.
- [x] Remove AI-service timeout and AiProxy service reference.
- [x] Add quota action schema and bounds.
- [x] Update CRDs and schema coverage tests.

## B. Controller

- [x] Validate and resolve EdgionAIService HTTPRoute backendRefs.
- [x] Add AI-service reverse dependencies and requeues.
- [x] Add HTTPRoute-to-EdgionPlugins reverse dependencies and requeues.
- [x] Add EBTP Service/AIService typed attachment indexes.
- [x] Implement per-target resolution and oldest-valid-wins arbitration.
- [x] Generalize EBTP ancestor status.
- [x] Preserve terminal AI refs, namespace, rule name, and UID through delegation.
- [x] Requeue EBTP ancestors for old/new AI targets on route and parent changes.
- [x] Resolve credential Secrets partial-good and update status.

## C. Gateway foundations

- [x] Compile typed route backends and invalid slots.
- [x] Audit and branch every Service-only backend consumer.
- [x] Build one typed destination-policy ArcSwap snapshot.
- [x] Preserve and clean stable destination-policy and credential-outlier handles.
- [x] Preserve and clean adaptive telemetry state handles.
- [x] Capture immutable request-local route, service, and policy snapshots.

## D. AI selection and retry

- [x] Build immutable canonical `AiAttemptPlan`.
- [x] Implement invalid-weight conformance gate.
- [x] Implement Weighted and Priority.
- [x] Implement LowestLatency and LowestUsage snapshots.
- [x] Implement CostWeighted fixed-point selection.
- [x] Implement credential-first bounded retry rounds.
- [x] Rebuild provider request and BodyStorage on every dispatch.
- [x] Integrate HTTPRoute retry, source retry budget, and commitment guard.

## E. Credential and resilience runtime

- [x] Add Redis RPM/TPM reservation and reconciliation.
- [x] Add credential Retry-After cooldown and rotation.
- [x] Generalize outlier state and half-open lease.
- [x] Apply AI-service circuit and connection policy.
- [x] Implement Throttle and Downgrade.

## F. Response, accounting, and observability

- [x] Add attempt-aware final response processor.
- [x] Add bounded per-attempt EdgionHttp access-log fields.
- [x] Reconcile failed and final attempt usage/cost.
- [x] Verify default-off token metrics and fixed label domains.
- [x] Add no hot-path tracing and no sensitive values.

## G. Verification and handoff

- [x] Run targeted resource, Controller, Gateway, and Redis tests.
- [x] Run AI integration scenarios and hot reload.
- [x] Run the AGENTS.md pre-commit suite and additional guards.
- [x] Complete implementation, review, coverage, execution, and wrap-up records.
- [ ] Stage local commits only; do not push.

### G2 notes

- Resilience 3/3 and CredentialExhaustion 2/2 passed (targeted `-r/-i`).
- No dedicated AI-3 hot-reload suite; SingleProvider DynamicTest secret-rotation remains the AI hot-reload coverage. Not inventing a new dynamic scenario in G.
- Fixed: `test_client` `"all"` path now includes `ResilienceTestSuite`; suite-map adds CredentialExhaustion row.

### G3 notes

- `fmt` / `check` / `clippy` / agent-docs / SSA / metrics inventory / pingora source policy — all exit 0 (2026-08-04).
- Clippy warnings only; not treated as failures.

### G4 notes

- Written: `07-changelog.md`, execution record in `02-implementation.md`, HANDOFF session log.
- Commit deferred until user explicitly requests.
