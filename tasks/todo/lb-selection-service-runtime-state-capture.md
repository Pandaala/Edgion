# Capture Service runtime health state once per load-balancer selection

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | performance refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Load-balancer selection captures the global health and outlier store handles once, but each candidate predicate still looks up the Service in both stores and then looks up the endpoint. Panic-threshold evaluation can scan all candidates once to count survivors and then scan candidates again for actual selection. Design a request-stable per-Service runtime view so one selection does not repeat global Service-map access for every candidate while preserving dynamic health, outlier recovery, and metric semantics.

## Requirements

- Capture the selected Service's active-health and outlier-detection runtime handles once at the start of one selection attempt.
- Candidate predicates must not repeat a global Service-map lookup, clone a Service handle, or rebuild endpoint identity for each health/outlier check.
- Compute each candidate's backend hash at most once per predicate evaluation and share it across active-health and outlier checks.
- Preserve panic-mode semantics: survivor counting does not increment rejection counters, actual non-panic rejection does, and lazy outlier recovery decrements the ejected-endpoint gauge exactly once.
- Keep health/outlier updates visible through an explicit immutable or atomically updated Service runtime handle; do not freeze mutable state for the lifetime of the process.
- Coordinate request-visible policy generation capture with `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md`, without forcing rapidly changing endpoint health into the policy configuration generation.
- Reuse the existing stores and selector cells. Do not introduce a third in-process cache implementation.

## Scope

### In scope

- `backends/selector.rs` health, outlier, and panic selection paths.
- Health-status and outlier-detection APIs needed to expose a safe per-Service read handle.
- Selection-path metrics and concurrency tests.

### Out of scope

- Health-probe scheduling or outlier ejection algorithms.
- Panic-mode gauge lifecycle, which is tracked in `tasks/todo/lb-panic-mode-metric-lifecycle.md`.
- Backend policy winner publication beyond coordination with its captured generation.

## Acceptance criteria

- Instrumentation proves that one selection attempt performs at most one top-level Service lookup in each health/outlier store, independent of backend count and panic-mode survivor scans.
- A candidate's backend hash is not recomputed between active-health and outlier checks.
- Concurrent health transitions and outlier ejection/recovery remain race-safe and eventually visible without rebuilding the selector.
- Panic and ordinary selection use compatible captured state and preserve the existing retry and not-found error mapping.
- Rejection, ejection, recovery, and panic metrics retain their documented meanings and cardinality.
- Hot-path code adds no tracing, blocking lock, or per-candidate heap allocation.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/00-common/05-unification-points.md`
- `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md`
- `tasks/todo/lb-panic-mode-metric-lifecycle.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer the per-Service runtime-handle redesign. | The local audit safely removed repeated global handle clones and duplicate hashes, but eliminating per-candidate Service lookups crosses independently updated health, outlier, panic, and metric state. |
