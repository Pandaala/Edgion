# B0071 — CtxSet Bounded Transform Execution

## Scope
- Baseline: `65511d86`
- Pipeline state: implementation independently approved, fully verified, and committed as `4ad0cf32`
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/ctx_set/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/ctx_set/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/ctx_set/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, resource/config and docs, PluginRuntime/PluginStore, session/context APIs, template engine, AccessLogExtern/redaction code, metrics/access-log code, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/ctx_set/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0070-csrf-expiry-runtime-ownership.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0071-ctx-set-bounded-transform-execution.md`; legacy follow-up record; legacy follow-up record; legacy follow-up index; standalone follow-up task; and standalone follow-up task

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `65511d86`; both target blobs and resource validation, KeyGet, template compilation/evaluation, session/context mutation, AccessLogExtern, conditional runtime, hot-reload, and test call chains remain unchanged.
- Baseline: 1 Gateway CtxSet test and 30 resource CtxSet tests pass.
- Static identity: each runtime instance allocates the constant plugin name.
- Default ownership: `Option::or(rule.default.clone())` clones the default even when the source value exists.
- Transform ownership: request processing materializes an owned `TransformType`, cloning configured transform strings before applying one operation.
- Substring allocation: the full input is collected into `Vec<char>` solely to clamp and slice one Unicode-scalar range.
- Decision: return the static name, clone defaults lazily, borrow the configured operation in the established priority order, and stream only the selected Unicode-scalar range into the output.
- Expected no-change file: `mod.rs` remains the minimal facade.

## Hard-rule boundary
- Preserve transform priority exactly: replace, substring, case, prefix, suffix, trim. Multiple configured operations continue using the first one until D0043 is separately resolved.
- Preserve precompiled regex use and the existing `transform regex missing` PluginLog plus rule-default graceful degradation; add no panic, debug assertion, or per-request regex compilation.
- Preserve KeyGet, canonical template evaluation without second-pass expansion, mapping/default semantics, rule order and cross-rule ctx reads, conditions, hot reload, exact PluginLog text/sensitive handling, and access-log declaration/masking/blocklist behavior.
- Preserve Unicode-scalar `[start,end)` substring semantics, clamping, empty ranges, and prefix/suffix output bytes.
- Add no tracing, metrics, access-log fields, public APIs, resource behavior, dependencies, integration-test edits, or duplicate template/provenance mechanisms.

## Deferred candidates
- Enforce exactly one configured TransformConfig operation in shared resource validation and align its documentation; this is separate from preserving current runtime priority in B0071.
- D0044: the user accepted the configuration-dependent sensitive ctx alias risk without a runtime change on 2026-09-11; see the [maintained decision](../../../../skills/04-review/observability/intentional-design/ctx-provenance-unmasking-risk.md).
- Existing integration tests remain frozen and already cover cross-process CtxSet behavior. Neither deferred boundary requires a new integration issue at discovery time.

## Implementation
- Complexity: simple; one private request-path ownership/allocation refactor with no public or control-flow contract change
- Implementer: `b0055_runtime_analysis`; distinct from the analyst and final reviewer
- Exact change: removed the allocated constant name, cloned the default only when source lookup misses, borrowed transform fields in the established priority order, selected Unicode scalar substrings through `skip`/`take`, and capacity-sized prefix/suffix outputs
- Test contract: 6 colocated tests cover static identity, source/default behavior, preserved multi-operation priority, Unicode substring boundaries, missing-regex graceful fallback, and the existing template-injection regression

## Independent review
- Code reviewer: `b0071_final_code_review`; distinct from analyst and implementer
- Code verdict: approved with no must-fix or should-fix findings; focused tests, Gateway compile, Clippy, no-tracing, diff hygiene, and frozen paths passed
- Historical deferred adjudication at B0071: D0043 was `needs-implementation` for exact one-operation validation; D0044 was `needs-discussion` for a cross-module sensitive-provenance design. Neither changed B0071 runtime compatibility or required a new integration issue at discovery. The later D0044 acceptance is linked above and does not establish that the defect was fixed.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 1 Gateway CtxSet and 30 resource CtxSet tests |
| Implementation and focused tests | passed | 6 colocated tests, 9 Gateway CtxSet tests, and 30 resource CtxSet tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | no must-fix or should-fix findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: `4ad0cf32`
