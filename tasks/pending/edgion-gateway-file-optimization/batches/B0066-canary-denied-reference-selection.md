# B0066 — Canary Denied-reference Selection

## Scope
- Baseline: `6cc29f5a6396391c0500ca860d9dfe86bde72ae8`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/canary/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/canary/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/canary/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, route manager/upstream selection, resources/Controller, metrics/access-log code, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0065 and B0066 batch records; reviewer-approved denied-reference deferred task records plus campaign backlinks; `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0055_runtime_analysis`, current at `6cc29f5a`; both target files are clean.
- Baseline: Canary tests pass 12/12, PluginRuntime tests pass 34/34, and HTTP upstream-peer tests pass 72/72.
- Authorization bug: Canary prevalidates the configured target through a local Service kind/name/namespace scan that ignores the Controller-resolved `ref_denied` marker. It can record Canary success and skip `onInvalid: reject`, while later named selection may swallow `RefDenied` and choose a legal sibling backend.
- Decision: reuse the canonical `RouteRules::find_backend_by_name` result so denied and missing targets both enter the existing `onInvalid` policy before any internal upstream jump is installed.
- Hot-path opportunity: active profile resolution allocates a profile-name `String` and then performs a second map lookup. Return the canonical borrowed map key and target from one `get_key_value` lookup instead.
- Expected no-change file: `mod.rs` retains the minimal private module and single live re-export.
- Preserved behavior: no-route/empty-backend pass-through, namespace matching, weight-zero explicit targets, selector fallback/logging, profile/Dye values, `onInvalid` response shape, fixed 403 body, PluginLog/access-log ownership, probability/header/cookie matching, and upstream jump semantics.

## Deferred candidates
- DynamicInternalUpstream repeats the local scan, and named upstream selection can swallow `RefDenied` into weighted fallback. This cross-file correction is not owned by the current Canary batch and is not covered by the L4-only denied-reference task.
- Existing integration tests remain frozen and already cover Canary selection, hot switching, invalid fallback/reject, Dye, and kill switch. The denied-reference contract is deterministic in a colocated test and needs no new integration issue.

## Implementation
- Implementer: `b0056_implementation`
- Complexity: simple; replace one local eligibility scan with the canonical finder and borrow one profile entry
- Plan: resolve the active profile as a borrowed canonical key/target pair, validate the chosen backend through the canonical route finder, and route every finder error through the existing `onInvalid` branch.
- Actual change: active profile resolution now borrows the canonical map key/target in one lookup; Canary validates the selected target with the canonical compiled-backend finder before publishing the jump or Success outcome, so denied and missing references both follow existing `onInvalid` behavior.

## Independent review
- Reviewer: `b0063_fresh_code_review`
- Verdict: Approved with no findings; fixture, denied-reference ordering, borrowed profile lifetime, and all existing selection/log/Dye semantics were independently verified.
- Deferred adjudication: D0034 owns the nonduplicate DynamicInternalUpstream and HTTP named-jump error-propagation correction; existing integration tests remain frozen and no new integration issue is warranted.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline Canary tests | passed | 12 tests |
| Baseline PluginRuntime tests | passed | 34 tests |
| Baseline upstream-peer tests | passed | 72 tests |
| Implementation and focused tests | passed | 13 Canary, 41 PluginRuntime, 72 upstream-peer, and 12 resource-filtered tests |
| Gateway all-target compile | passed | implementation and review verification |
| Independent code review | passed | no findings; one-file scope and frozen integration coverage confirmed |
| Full campaign gates | passed | format, workspace check, workspace Clippy, agent docs, SSA, metrics inventory, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | existing integration tests unchanged from the user-authorized baseline |

## Handoff
- Ledger/changelog updated: yes; one changed and one examined-no-change
- Commit: pending
