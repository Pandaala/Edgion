# B0129 — Plugin Runtime Log

## Scope

- Baseline: `2cae2cfc59616782d5833ff8920e7c2158ba6bb1`
- Pipeline state: terminal; verified, awaiting local commit
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/log.rs`
- Baseline blob: `9de194c0c7d9c16ecd20beb9964653da67b7c60c`
- Proposed exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/log.rs`
  - `edgion-resources/src/sensitive.rs`
  - `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` (LogFmt parameter preparation only)
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
- Freshness: `b0129_freshness` revalidated at `b71f37f2` with the verified B0128 worktree changes. The original three paths are unchanged from `2cae2cfc`; B0128 does not affect this call chain.
- Scope correction: the analyst requires the LogFmt branch in `builtins.rs` because `runtime_string` copies each shared `Arc<str>` into `Vec<String>` before the sink. A valid 254-argument call can duplicate a repeated 1 MiB string roughly 254 times, including Replace mode. Borrow string contents while retaining Value ownership; preserve non-string coercion and every Body TypeError even when output overflows. This directly supports the same bounded-log value case, not independent DSL cleanup.
- Additional invariants: reject an oversized entry atomically, preserve tier ordering and later small entries after `log_full`, and judge Replace by rendered size. Bounded Wrap must avoid growing retained thread-local storage before the sink checks. Run mode-mutating aggregate tests with `--test-threads=1`.
- Knowledge: log safety, canonical access-log buffer/API, plugin-system ownership, DSL engine, builtin error semantics, declined DSL execution tracing precedent, and unit-testing rules, progressively routed through the project index.
- Implementer: `b0129_implementation`; approved plan implemented in the four allowed files. Bounded masking rejects before TLS growth, capped formatting preserves whole-entry/tier semantics, and DSL LogFmt retains shared string Arcs while preserving scalar and Body coercions.
- Plan: (1) share masking dispatch with a bounded sensitive helper that checks rendered size before TLS growth; (2) bound every template append to 512 bytes; (3) preserve overflow tier initialization through the existing sink and reject entries atomically; (4) retain/move DSL string Arcs and preserve all scalar/Body coercions and order; (5) cover exact limits, scratch/TLS capacity, tier ordering, shared DSL strings and late errors; (6) run serial focused tests before independent final review/full gates.
- Plan review: `b0129_plan_review` APPROVED, no Must Fix or Should Fix. Use the canonical ceiling, preserve unknown-mode Wrap and normal callback cleanup, distinguish allocator capacity rounding from logical byte bounds, and source the large DSL value at runtime with explicit valid limits.
- Review: `b0129_final_review` LGTM, no Must Fix or Should Fix. It independently verified boundedness, shared redaction dispatch, exact tier transitions, Arc ownership and all coercions, plus exact error variants and high-arity test limits. Allocation evidence is structural/capacity-based, not a profiler measurement.
- Verification: resources sensitive 13/13, Gateway runtime log 29/29, DSL p3 39/39 passed serially; format and diff checks passed. New fixture wiring/expected-log setup was corrected without production changes. Locked workspace check (16.65s), Clippy (23.94s, existing warnings), Gateway binary build (39.80s), agent-doc/SSA/metrics/Pingora/no-tracing/English-only and frozen-path guards passed.
- Commit: `b6060c7a` (recovery commit); not pushed.

## Verified worktree fingerprints

| File | Blob |
|------|------|
| `edgion-gateway/src/plugins/runtime/log.rs` | `fff452140b36f107856e3458c5d81d0e475e3e95` |
| `edgion-resources/src/sensitive.rs` | `7f7ddf4b8727a104ee62b0c256220d9a0e6dd927` |
| `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` | `2d4a52e8a5c47ea649780fde02eb9a4a406665b0` |
| `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs` | `2d9864280c207cb3ce7e832e73facb78d9c6fc31` |
