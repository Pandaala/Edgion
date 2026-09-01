# B0074 — DynamicInternalUpstream Borrowed Selection and Accepted-jump Formatting

## Scope
- Baseline: `46e3ec98`
- Pipeline state: implementation independently approved, fully verified, and committed as `42b25fca`
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/dynamic_internal_upstream/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/dynamic_internal_upstream/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, shared resource/config and docs, routing/upstream peer and backend selection, PluginRuntime/PluginStore, jump/session/context APIs, metrics/access-log implementation, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0073-dynamic-external-upstream-jump-ownership.md`; and `tasks/pending/edgion-gateway-file-optimization/batches/B0074-dynamic-internal-upstream-borrowed-selection.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `46e3ec98`; target blobs are `8eb292ec75064cdfab51063e03aa49b4e9776b25` for `mod.rs` and `f1130c8c2d7e227b8dd8fde6dff6096fd3c56043` for `plugin.rs`.
- Baseline: 3 Gateway DynamicInternalUpstream, 6 resource, 34 PluginRuntime, and 72 HTTP upstream-peer tests pass.
- The constant plugin name is allocated per instance, regex capture text is copied before lookup, and selected name/namespace are copied before building the owned request-context preset.
- Dye display cloning/formatting also occurs before first-write-wins ownership is known, although this plugin has no pre-jump request mutation equivalent to B0073.
- Decision: return a static name, retain capture and selection values as borrows until the preset ownership boundary, and construct the Dye display only after `SetJumpOutcome::Set`.
- Expected no-change file: `mod.rs` remains the minimal module/export facade.

## Hard-rule boundary
- Preserve canonical KeyGet, regex extraction, rule order, namespace matching, weight-zero behavior, backend mapping, immutable hot reload, and the owned `InternalJumpPreset` stored in request context.
- Preserve gRPC skip, missing/regex-miss/no-match behavior, onMissing/onInvalid fallback/reject policy, fixed client bodies, exact PluginLog and Dye cause/value semantics, and first-write-wins.
- Preserve ordinary compiled-backend ownership of BackendTLSPolicy, EndpointSlice/load balancing, address safety, timeout, and retry behavior.
- Do not change raw backend-ref scanning or `pick_backend_ref` error handling. D0034 owns the cross-file denied-reference fail-close correction; a plugin-only change would be incomplete.
- On `Rejected`, publish no Success Dye and avoid target display cloning/formatting; retain exactly `jump locked, rejected`.
- Add no tracing, metrics, access-log fields, schema behavior, public APIs, dependencies, integration-test edits, or duplicate backend selection mechanisms.

## Deferred candidates
- D0034 / `fail-closed-on-denied-l7-named-jumps.md` already owns denied-reference behavior across this plugin and `pg_upstream_peer.rs`; do not duplicate or partially implement it.
- Regex DFA limits remain an accepted precedent. No new deferred finding is required.
- Existing frozen integration coverage has 11 cases for direct/rule selection, missing/no-match/invalid policies, weight-zero, Dye/access log, and stable selection. Colocated ownership tests are sufficient, so no new integration issue is required.

## Implementation
- Complexity: simple; private borrowing and accepted-path formatting only
- Implementer: `b0055_runtime_analysis`; distinct from analyst and final reviewer
- Exact change: returned a static name, borrowed regex and selected routing values until the owned preset boundary, and created namespace/name success output only after an accepted jump
- Test contract: static name identity; regex capture with namespace preset and Success Dye; no-namespace Success value; rejected jump with no Dye and exact locked log; preserved missing Reject and invalid Fallback

## Independent review
- Code reviewer: `b0074_final_code_review`; distinct from analyst and implementer
- Code verdict: approved with no must-fix findings; confirmed borrow ownership, direct and rule selection, namespace/weight-zero behavior, exact Dye values, rejected-jump behavior, and failure-policy stability
- Deferred adjudication: existing D0034 remains the sole owner of raw-scan and upstream-selector denied-reference behavior; no new deferred or integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 3 Gateway, 6 resource, 34 PluginRuntime, and 72 upstream-peer tests |
| Implementation and focused tests | passed | 6 Gateway, 6 resource, 34 PluginRuntime, and 72 HTTP upstream-peer tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | no must-fix findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: `42b25fca`
