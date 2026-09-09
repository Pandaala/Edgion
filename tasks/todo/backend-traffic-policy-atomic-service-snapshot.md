# Publish Service backend traffic policy through one immutable snapshot

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | implemented |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

This task is recorded as implemented; the following describes its original problem and acceptance contract, not a claim that the defect persists. Request-visible Service policy fields were published through separate maps/stores, allowing mixed generations; authority/load-balancing-only updates could fail to advance the typed snapshot. Reconciliation also scanned all policies per affected target despite unbounded Service fan-out. The required correction is one typed immutable policy/index generation, captured once by request consumers. Revalidate current code before reopening any part of this completed work.

## Requirements

- Put every request-visible Service policy value in one `EffectiveBackendTrafficPolicy` generation keyed by typed `PolicyTargetKey`.
- Capture the effective Service policy once at the defined request or attempt boundary and use that generation for load balancing, authority, retry budget, circuit breaker, connect timeout, and panic threshold.
- Make load-balancer-only and authority-only changes advance the immutable generation and retain old captured generations for in-flight requests.
- Keep health-check task reconciliation and other background side effects ordered, but do not use their mutable stores as the canonical request-configuration source.
- Preserve oldest-creation-time-then-name winner selection, typed Service/EdgionBackend isolation, equivalent-config `Arc` retention, and clear-before-add behavior.
- Add one reverse index from typed target to eligible candidate policies, published with the policy map, so full and partial updates do not perform an `affected_targets × all_policies × target_refs` scan.
- Precompute validation eligibility, canonical target keys, and precedence data once per policy generation rather than allocating them in every target resolution.
- Do not create a second independent index or expose a mixed-generation policy/index pair.

## Scope

### In scope

- `BackendTrafficPolicyStore` publication and lookup model.
- Core Service request consumers and typed effective-policy capture.
- Health, outlier, degrade-threshold, and authority reconciliation ordering needed for the new source of truth.
- Full and partial conf-sync publication tests.

### Out of scope

- `EdgionBackendTrafficPolicy` schema changes or Service fan-out limits.
- Route destination selection or weight semantics.
- BackendTLSPolicy lookup semantics.

## Ownership Contract

Service policy capture occurs at the first `begin_upstream_attempt`, without a
`selected_backend` gate that would omit gRPC. Publish the immutable snapshot
before reconciling probes; equivalent generations skip redundant side effects.
Keep live connection counts on existing runtime objects and distinguish their
state from configured panic thresholds. Candidate target keys are deduplicated;
retarget/removal must patch the reverse index without losing unaffected entries.
Any follow-up must review publication, capture/readers, index mutation and
background side effects together; no separate progress or plan document is a
prerequisite for understanding this contract.

## Acceptance criteria

- Concurrent publication tests cannot observe a combination of old and new request-visible Service fields.
- Load-balancer-only and authority-only updates advance the effective generation exactly once.
- Captured pre-update Service and AI generations remain immutable and typed targets with equal names never collide.
- Full and equivalent partial updates publish the same policy and index state.
- Retarget and removal clear old reverse-index entries and all derived side effects.
- Reconciliation complexity is proportional to changed policies, their old/new targets, and candidate policies for those targets rather than all policies for every target.
- Retry accounting for one Service attempt uses the captured budget generation documented in [retry-budget epoch coherence](../../skills/04-review/backends/regression-guards/retry-budget-epoch-coherence.md).
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/07-backends.md`
- `skills/01-architecture/05-resources/20-edgion-backend-traffic-policy.md`
- `skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`
- `skills/04-review/backends/regression-guards/ebtp-ai-target-limits-preserve-service-fanout.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer implementation from the file audit. | Removing one compatibility map or adding a standalone index would leave mixed-generation readers; correctness requires migrating all request consumers and publication state together. |
| 2026-09-04 | Review snapshot and capture as one design. | Publication, consumer capture, incremental reverse-index mutation, outbound readers and L4 panic-state ownership must agree. |
| 2026-09-05 | Incorporate publication, capture and side-effect reviews. | Preserve deduplicated target keys, equivalent-generation reuse, AI outlier handle ownership and Service/gRPC capture coverage; these invariants are stated above without depending on separate plan numbering. |
