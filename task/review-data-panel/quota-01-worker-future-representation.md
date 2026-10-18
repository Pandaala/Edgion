# Quota worker future representation

Status: deferred optimization for user decision. No runtime change.
Source baseline: 2ebff22f1.

## Evidence

`ai/quota/mod.rs` defines QuotaFuture as a pinned boxed Send future.
QuotaSink returns this future, while AiQuotaRequest::start and run_worker retain
an Arc<dyn QuotaSink>. The sole production implementation, RedisQuotaSink in
`ai/quota/runtime.rs`, boxes apply_all on each invocation. Two quota test sinks
and one HTTP response test sink implement the same seam.

The response-body path updates the shared accumulator and notifies the worker;
it performs no Redis IO. The worker invokes the sink for threshold/timer flushes,
lease refreshes and final reconciliation. Its accumulator Arc is independently
owned by the context handle, spawned worker and response processor. These
cross-task owners cannot be replaced by short synchronous borrows.

Generic sink dispatch returning a concrete Send future could remove the
per-flush box and dynamic dispatch. It also changes the spawned worker's concrete
state-machine layout: the in-flight operation becomes part of that state instead
of residing behind a box. This may increase persistent worker-frame residency
between sparse updates. The net frame size, allocation bytes, retention and CPU
impact are unmeasured; no regression or overall improvement is established.
The current apply_all future includes a timeout and concurrent per-target
join_all. Removing its result container or serializing targets is a separate
change, not permission granted by this proposal.

## Decision

Choose whether to retain the present representation or investigate generic direct
dispatch with explicit frame/allocation measurements. There is no compatibility
requirement to keep this pre-release public trait object shape, and test convenience
alone does not justify hot-path overhead. Conversely, avoiding one transient box
does not establish a lower per-request or process-wide memory footprint.
A larger persistent frame would need an explicit cost/benefit decision.

## Acceptance for a selected implementation

- Compare size_of_val for representative production worker/apply futures, task
  allocation requests, peak live allocation and idle residency for sparse and
  frequent updates. Include one and several quota targets. Separate dependency
  allocations from the eliminated box; use the same build profile and toolchain.
- Measure completed, failed, hanging, cancelled and unknown-final operations.
  A benchmark using only trivial test futures cannot establish production layout
  or Redis-path CPU costs. No new production metric is needed.
- Preserve one worker per quota-enabled request, shared accumulator ownership,
  monotonic observations, coalesced notifications and sticky over-limit state.
- Preserve independent threshold and lease timers, cumulative revision order,
  attempted sent-high-water before await, unknown-final conservative retention,
  downward authoritative correction and the single total final-retry deadline.
- Preserve request-drop abort, timeout abort, completed-worker join and cancellation
  of in-flight target operations. Do not introduce detached work or body-path IO.
- Keep per-target concurrency, Redis timeout/failure behavior, fixed key ownership,
  and canonical shared executor/NOSCRIPT semantics unchanged.
- Retain existing worker lifecycle, quota accounting and HTTP response tests;
  update all sink implementations together without keeping an unused legacy API.
  Freeze an exact implementation allow-list under a new IMPL plan before edits.

Relevant sources: `ai/quota/mod.rs`, `ai/quota/runtime.rs`,
`ai/runtime/dispatch.rs`, `tests/unit/ai/quota/mod_test.rs`, and
`tests/unit/ai/http/response_test.rs` in edgion-gateway. The canonical quota
settlement and monotonic streaming-usage review knowledge remains authoritative.
