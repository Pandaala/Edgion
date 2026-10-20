# Stage-specific plugin log capacity policy

Status: deferred memory and preparation tradeoff for user decision. No runtime change.
Source baseline: 7cfa92726.

## Evidence and decision

In edgion-gateway/src/plugins/runtime/plugin_runtime.rs, the request runner starts
filter_logs empty and reserves filters.len() only when the first unsuppressed log
is ready. Early RequestId execution passes the full request_plugins slice but
skips non-RequestId entries; AfterEarly skips RequestId entries. The reservation
therefore includes entries excluded by the phase. The response sync/async runners
likewise reserve their full list on the first unsuppressed result. Suppressed
entries and termination can reduce the number of logs actually retained.
Vec::reserve guarantees at least the requested capacity, not exact heap bytes.

This lazy policy already avoids allocating the top-level log vector for an empty
or entirely suppressed dispatch. It does not establish that the whole dispatch
is allocation-free. The unavailable-IP replacement uses its selected gate slice;
capacity changes must not accidentally count the original list instead.

finish_plugin_stage in ctx.rs moves filters into StageLogs and moves pending
resource logs with mem::take. Unused vector capacity lives with those stages.
AccessLogEntry borrows the stage slice; to_json creates the owned serialized output.
There is no full stage-log clone to remove at this handoff. Terminal logging's
optional integration-store JSON clone has a separate second-consumer purpose.

Evaluate retaining the current policy against phase-specific prepared upper
bounds or incremental growth. Prepared counts add cold state and require upkeep
across every runtime construction/sealing path; per-request counting adds another
traversal. Counts remain upper bounds when a plugin terminates execution. Smaller
initial reservations can reduce retained unused slots but increase reallocations
on long successful chains. No net heap or throughput improvement is established.

As a supporting comparison, ExtensionRef inner logs currently grow from Vec::new
through session_adapter::push_to_edgion_plugins_log. They are owned by token-indexed
pending resource entries. Eagerly reserving the whole inner list can worsen empty,
suppressed or early-terminated cases. Do not redesign token ownership merely to
unify these different reservation policies.

This is distinct from [early runtime-list preparation](early-01-precompute-plugin-lists.md),
which addresses publication-owned executable arrays and request-time resolution.

## Acceptance for a selected implementation

Preserve log order, stage boundaries, names, aliases, conditions, timing fields,
redaction, empty-stage omission and pending-resource grouping. Preserve RequestId
phase selection, IP/config/policy gates and termination outcomes. Native WAF keeps
its dedicated summary; body callbacks remain log-less. Keep final attached-summary
mutation before terminal serialization and the existing debug snapshot semantics.
Add no tracing, metrics, request locks or alternative log cache.

Measure actual allocations, reallocations and retained capacity across empty,
single/large mixed RequestId chains, suppressed-only/mixed chains, gate replacement,
first/middle/last termination and successful completion. Compare nested ExtensionRef
separately. Account for runtime metadata, overlapping generations and long-lived
requests; allocator requests alone do not establish RSS or throughput savings.
Validate construction and fail-closed sealing paths for any prepared count. Obtain
independent IMPL plan/final review and project checks before implementation.

Original contracts: skills/01-architecture/02-gateway/05-plugin-system.md,
skills/03-coding/observability/00-access-log.md, and
skills/04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md.

Fresh baseline: 542 plugin-runtime, 18 access-log entry, 11 terminal-logging and
eight plugin-request-state tests passed. The ninth state test reproduced the
existing 184-versus-176 layout assertion failure tracked in
[verification-04](verification-04-plugin-state-layout.md). No tests were changed
or disabled. These results do not measure the proposed capacity alternatives.
