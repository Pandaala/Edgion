# Resolve local limiter initialization cost without shifting admission windows

Status: deferred for user decision. Priority: P2. Type: performance/design investigation.
Inspected baseline: `8458c4dbb`. No runtime change.

## Evidence and design tension

RateLimitLocal::run_request resolves its key before get_rate initializes the
per-plugin OnceLock<Rate>. Missing-key Allow or Deny returns before initialization.
The first admitted key path constructs the estimator; concurrent first callers
can wait for that initialization. No limiter state is shared between plugin
instances.

The pinned Edgion Pingora revision 1aaa45cc70430bc782990bc20ddee3fef36d6b99,
pingora-limits/src/rate.rs, constructs two Estimator matrices and sets start to
Instant::now(), last_reset_time to zero and the current slot to red.
Estimator::new allocates four row buffers plus outer storage for each matrix
when called with the Gateway's four hashes. This is not merely two allocations.
Rate::observe calls maybe_reset before incrementing the current estimator.

The graded design rules, section U, require reusable work to be prepared off the
request path. Current lazy construction is evidence of an unresolved tension,
not an accepted exemption from that rule. Moving Rate::new into the plugin
constructor does remove first-use construction work, but also moves its clock
anchor and eagerly retains memory for instances that may never be used.

## Why a direct move is not behavior-preserving

Source-derived example, not a timing benchmark: choose a one-second interval,
a threshold of one and one isolated key. With configuration construction at t=0,
requests at t=0.6 and t=1.1 produce different results. Lazy construction starts
near the first request; the second request is still in its first interval and
observes count two. Eager construction starts near configuration time; the second
request resets the window and observes count one. The current greater-than-limit
comparison can therefore deny versus allow.

This is not universal phase drift. If the first observation occurs after an
interval, maybe_reset sets last_reset_time to that observation and flips slots.
The first-observation delay, rollover and concurrency cases must all be modeled.
Do not hide the issue by choosing only tests where startup delay exceeds a window.

## Decision and acceptance

Choose a reviewed design that separates cold storage preparation from first-use
time ownership, or explicitly decide whether changed window semantics and eager
memory ownership are acceptable. Retaining the current implementation pending
that decision does not close the cold-initialization design requirement.

The pinned Rate API presently constructs storage and time together. A design
requiring a new fork seam needs a separate explicit dependency allow-list and
review; this task authorizes no Pingora edits, reset shortcut or private-field
workaround. Do not duplicate the CMS algorithm or introduce a shared global
limiter to avoid per-instance construction.

Measure cold construction latency and allocations across configured estimator
sizes, first-use concurrency, unused instances, overlapping reload generations
and steady-state overhead. Verify whether a prepared-storage design actually
moves expensive work off the request path while retaining its intended epoch.
No alternative implementation, allocation probe or latency benchmark ran.

Preserve key-resolution order and missing-key behavior, independent plugin state,
attempt counting (including excess), greater-than-limit comparison, Continue vs
Reject, outcome-slot publication and dependent Redis admission. Effective rate
in Cluster scope reads live gateway instance count; it must not be frozen merely
because estimator storage becomes prepared. Preserve interval/slot validation,
limits and configuration replacement ownership.

A future IMPL must test first observation before, at and after an interval,
subsequent rollover, simultaneous first callers, no-traffic and missing-key
instances, repeated replacement and dropped old generations. Keep request-path
observability within existing access-log/outcome mechanisms; add no tracing or
per-key metric. Scope tests to actual Rate semantics and real plugin wiring.

Authority/evidence: `skills/00-design-rules.md` section U,
`skills/04-review/rate-limit/regression-guards/local-allowance-redis-overflow.md`,
Gateway rate_limit_local/plugin.rs and the pinned rate.rs/estimator.rs sources.
Fresh unchanged-code verification passed 17 Gateway rate_limit_local-filtered
tests, formatting check, workspace check/Clippy and agent-doc validation.
Existing unrelated warnings remain. No full suite or live integration ran.
