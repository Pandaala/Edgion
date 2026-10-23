# AI scheduler initialization boundary

Status: deferred optimization/API decision for user review. No runtime change.
Source baseline: `143cfe3b3`.

## Evidence

`ai/http/proxy.rs` constructs AttemptScheduler before quota admission can await.
It validates backend/credential bounds and the route retry cap at this boundary.
An absent route cap initially means zero retries; continuation uses zero without
reading the route cap. Quota downgrade later changes candidate model/adaptive
state, not captured backend credential weights or scheduler progress.

`ai/http/attempt.rs::select_peer` passes the route cap and gateway fallback into
`ai/runtime/dispatch.rs::prepare_first_attempt`. After the deadline gate, dispatch
resolves an explicit cap strictly (maximum seven), or clamps the global fallback
to seven. This resolution runs on every call, before initialize_retry_cap's
once-only check. Thus invalid explicit caps can still fail on later calls and
are validated even for continuation before its zero-retry override.

The first initialize_retry_cap rebuilds the scheduler from captured candidate
credential rows and marks initialization complete. Later calls preserve the
resolved budget, exclusions, dispatch count and failure history. The constructor
initializes fixed storage for 16 backends, 16 credential weights per backend and
eight failure entries, copying present weights. The preceding iterator change
already removed temporary weight/projection Vecs at both sites. This remaining
candidate concerns bounded repeated initialization/copying, not temporary heap
containers, a proven bottleneck or a correctness finding.

## Decision alternatives

- Retain the two constructors if their bounded cost does not justify a new
  lifecycle interface.
- Finalize the cap on an already validated pristine scheduler through a narrow
  interface. Define and enforce the permissible phase, repeated-call behavior
  and error behavior; zero dispatches alone do not prove no selection or skip
  progress. Do not introduce an unrestricted budget setter or silently preserve
  state that the current first initialization replaces.
- Construct only at dispatch after preserving early validation independently.
  Prove that this does not move malformed-configuration rejection behind quota
  I/O or introduce a second validation implementation, optional-state ambiguity,
  another scheduler owner or changed failure cleanup.

No alternative is selected. Early global-cap resolution is also a lifecycle
change requiring evidence of the same configuration source and observation time.
Do not cache live eligibility, refresh backend generations or fold unrelated
quota/model selection into this change.

## Acceptance

Preserve strict explicit versus bounded fallback caps, absent/zero/seven caps,
invalid explicit caps, continuation's one-dispatch ceiling, and per-call cap
validation before the initialized fast path. Cover first initialization and
later calls after credential/service skips and actual dispatch progress.
Preserve constructor bound/error precedence and pre-quota rejection, captured
backend generations across waits, downgrade/readmission, deadline precedence,
dispatch marking, retry-budget accounting and reservation release/cancellation.
Tests must cross request-planning and dispatch seams, not just the pure scheduler.

Measure representative and maximum candidate/credential counts and generated
initialization work before choosing an API. Source inspection alone establishes
no throughput, allocation, RSS or async-future-size improvement. Keep runtime
ownership, fixed bounds, body policy, redaction and observability unchanged.
Freeze implementation scope and obtain independent IMPL plan/final review.

## Original knowledge and verification

- [AI runtime ownership](../../skills/01-architecture/02-gateway/00-overview.md#ai-implementation-boundary)
- [Failover boundary tests](../../skills/04-review/ai-routing/regression-guards/ai-failover-tests-must-cross-runtime-boundaries.md)
- [Predispatch deadline gates](../../skills/04-review/ai-routing/regression-guards/ai-predispatch-scan-rechecks-deadlines.md)
- [Service-scoped predispatch failures](../../skills/04-review/ai-routing/regression-guards/ai-endpoint-failure-skips-the-service.md)

Fresh unchanged-source tests passed 11 scheduler, 36 dispatch and 22 proxy tests
(69 total). They verify existing behavior, not a proposed initialization API.
No prototype, benchmark, full suite or live integration ran for this audit.
