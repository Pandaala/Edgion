# B0129 — Plugin Runtime Log

## Scope

- Baseline: `2cae2cfc59616782d5833ff8920e7c2158ba6bb1`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/log.rs`
- Baseline blob: `9de194c0c7d9c16ecd20beb9964653da67b7c60c`
- Proposed exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/log.rs`
  - `edgion-resources/src/sensitive.rs`
  - `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs`
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete plugin runtime log representation, every producer and consumer, final access-log attachment, boundedness/redaction behavior, tests, and relevant review knowledge.
- Preserve canonical access-log ownership, binary-owned operational logging, sensitive-data policy, bounded request state, immutable publication, observability restraint, and response streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change. `PluginLog::push_fmt` is the only entry path that materializes an entire formatted value before the 512-byte persistent sink rejects it. DSL `log_fmt` can supply request-derived sensitive values up to the configured 1 MiB per string and many arguments, causing disproportionate transient allocation and Wrap-mode thread-local capacity retention on the request path despite bounded stored state.
- Implementation direction: add one canonical bounded sensitive-render helper, stop incremental template rendering at the exact sink ceiling without retaining a partial entry, and preserve byte-identical accepted outputs plus the existing `log_full` transition in Off/Replace/Wrap modes. Add exact 512/513, huge plain/sensitive, long-input-short-Replace, Wrap capacity, UTF-8, no-partial-write, and DSL regressions.
- Complexity: complex and plan-first because the change touches the request hot path and canonical sensitive rendering boundary.
- Proposed scope: the owned runtime log, shared sensitive helper, and one existing crate-local DSL unit-test file. Existing integration files remain unchanged and no integration issue is needed.
- Deferred finding: [`D0094`](../deferred/D0094-dsl-builtin-argument-count-encoding.md) owns unchecked variadic builtin argument-count narrowing in the DSL compiler.
- Review: pending.
- Verification: pending.
- Commit: pending.
