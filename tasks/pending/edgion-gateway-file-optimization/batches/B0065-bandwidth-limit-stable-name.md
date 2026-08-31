# B0065 — BandwidthLimit Stable Plugin Name

## Scope
- Baseline: `2cb738460ac1142a46aa1da96dd04627c8eb244d`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/bandwidth_limit/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/bandwidth_limit/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/bandwidth_limit/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, response-body traits/runtime/dispatch, resource schemas, metrics/access-log code, docs, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0064 and B0065 batch records

## Context and value gate
- Analysis subagent: `b0056_final_review`, revalidated against `2cb73846`; both target blobs and the relevant trait/runtime/resource call chains remain unchanged.
- Baseline: Gateway BandwidthLimit tests pass 2/2 and resource parsing/validation tests pass 9/9.
- Concrete cost: every compiled plugin generation allocates and retains `format!("BandwidthLimit({})", config.rate)` even though production response-body dispatch never calls the name, the body stage emits no per-chunk PluginLog, and entry alias is the canonical instance discriminator.
- Decision: remove the unused owned name and return the stable type name `"BandwidthLimit"` from the trait method.
- Expected no-change file: `mod.rs` retains the minimal private module and single live re-export.
- Preserved behavior: validated fail-close construction, direct invalid-construction sentinel, bytes/rate duration math, empty/None/EOS behavior, per-stream scope, max-delay combination, streaming backpressure, and zero-log/zero-allocation per-chunk execution.

## Deferred candidates
- None. Existing integration tests remain frozen and already cover real latency/metrics behavior; this construction invariant needs no future integration issue.

## Implementation
- Implementer: `b0056_implementation`
- Complexity: simple; remove one dead field/allocation and add a colocated stable-name regression
- Plan: keep only `rate_bps`, return the static plugin type name, and preserve every throttle and validation path.
- Actual change: removed the retained dynamic name and formatting allocation; the plugin now stores only `rate_bps` and returns the stable static type name.

## Independent review
- Reviewer: `b0063_fresh_code_review`
- Verdict: Approved with no findings; alias remains the canonical instance discriminator and every throttle/failure path is unchanged.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline Gateway tests | passed | 2 tests |
| Baseline resource tests | passed | 9 tests |
| Implementation and focused tests | passed | 3 Gateway, 9 resource, and 1 runtime invalid body-stage fail-close test |
| Gateway all-target compile | passed | implementation and review verification |
| Independent code review | passed | no findings; one-file scope and existing integration coverage confirmed |
| Full campaign gates | passed | format, workspace check, workspace Clippy, agent docs, SSA, metrics inventory, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | existing integration tests unchanged from the user-authorized baseline |

## Handoff
- Ledger/changelog updated: yes; one changed and one examined-no-change
- Commit: pending
