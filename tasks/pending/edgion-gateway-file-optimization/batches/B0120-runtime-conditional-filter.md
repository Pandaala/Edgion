# B0120 — Conditional Plugin Wrappers

## Scope

- Baseline: `86ea7498`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/runtime/conditional_filter.rs`
- Baseline blob: `10b59131013f59e709148d06d71562c4684818d3`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read every wrapper stage, shared `Condition` evaluation path, PluginLog/access-log behavior, runtime construction/publication, request/response body semantics, tests, and review precedent.
- Preserve condition combination semantics, stage ordering, failures, immutable generations, redaction, observability restraint, response streaming/backpressure, and canonical predicate ownership.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change. Production behavior is correct, conditions are precompiled, logged stages use value-free `PluginLog.cond_skip`, and the response-body wrapper remains streaming and log-free per chunk. The concrete gap is test strength: the request mock's counter moves into the box and is never observed, so existing run/skip cases prove results but not delegation. The other three wrapper stages lack focused run/skip delegation tests. Rustdoc also names obsolete `skipped_by_condition` instead of `cond_skip` and overstates empty-set runtime behavior even though validation rejects empty sets before publication.
- Candidate correction: keep production structure unchanged; correct the two wrapper contract comments, expose mock invocation counters through `Arc<AtomicUsize>`, assert exactly one call for run and zero for skip, and add focused run/skip coverage for synchronous response headers, response chunks including returned duration, and asynchronous responses.
- Preserved behavior: canonical compiled `Condition` ownership and exact skip/run semantics, no hot-path compilation/allocation/lock changes, response streaming and single request `BodyStorage`, log-free chunk filtering, alias/dye/suppress-log forwarding, value-free diagnostics, and existing failure policy.
- Complexity: simple; exact implementation allow-list is only `edgion-gateway/src/plugins/runtime/conditional_filter.rs`, including its inline tests and Rustdoc.
- Freshness: analyzed at `86ea7498`; target blob exactly matches `10b59131`.
- Deferred disposition: evaluator work belongs to B0121; body behavior belongs to B0119; response-body no-work/dispatch gaps already have canonical tasks. No new deferred issue.
- Integration disposition: no integration issue is needed. Existing tests remain frozen, and transparent wrapper delegation can be fully verified with focused unit tests without publication/hot-reload changes.
- Review: pending.
- Verification: pending.
- Commit: pending.
