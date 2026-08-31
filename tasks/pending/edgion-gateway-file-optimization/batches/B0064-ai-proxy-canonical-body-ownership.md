# B0064 — AI Proxy Canonical-body Ownership

## Scope
- Baseline: `1551f1ce4265abe387a121f0eb92a1fb5f536755`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/ai_proxy/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/ai_proxy/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, BodyStorage and PluginSession APIs, AI dispatch/quota/runtime code, resource schemas, metrics/access-log code, docs, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0063 and B0064 batch records

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `1551f1ce`; both target blobs and relevant call chains remain unchanged from the queued report.
- Baseline: AiProxy colocated tests pass 12/12.
- Concrete hot-path cost: `PluginSession::read_request_body` already returns an Arc-backed `Bytes` handle by value, but `canonical_openai_request` borrows it and the unchanged-body branch calls `Bytes::copy_from_slice`, adding an O(body bytes) allocation and memcpy before `ReadyAiAttemptPlan` stores another `Bytes` owner. The unified physical request-body cap defaults to 32 MiB and is operator-configurable.
- Redundant validation: the helper performs a full `from_utf8` scan before `serde_json::from_slice`, although serde JSON parsing already rejects invalid UTF-8 with the same `MalformedBody` classification; the final scalar-count scan remains required for usage/quota semantics.
- Decision: consume the body `Bytes`, move it into the canonical request when no usage-field rewrite occurs, and allocate only when serialization actually changes the JSON.
- Expected no-change file: `mod.rs` retains a minimal private module and single live re-export.
- Preserved behavior: one shared request `BodyStorage`, body generation checks, canonical wire bytes, streaming usage injection, rewrite/model routing, usage character count, quota, retry plan, client errors, PluginLog/access-log/redaction, metrics, failure policy, and response streaming.

## Deferred candidates
- None. Existing dynamic AI integration tests remain frozen and already cover behavior; backing-allocation reuse is a unit-level ownership invariant and needs no future integration issue.

## Plan-first implementation
- Implementer: `b0055_runtime_analysis`
- Complexity: small diff but plan-first because it changes ownership at the canonical request-body boundary
- Required plan review: approved independently by `b0056_final_review`
- Approved plan: consume the returned `Bytes` only after the body-generation guard, parse through serde without the redundant UTF-8 pass, serialize a new body only when usage injection mutates JSON, otherwise move the exact original view, and keep final Unicode-scalar accounting and all downstream plan semantics unchanged.
- Test contract: a nonempty sliced `Bytes` with a live witness must retain pointer, length, and bytes on the unchanged path; usage injection keeps semantic JSON assertions primary and may additionally prove a different pointer while the source witness lives; invalid UTF-8 remains `MalformedBody`.
- Actual change: the canonical helper consumes the guarded request `Bytes`; serde performs the only initial UTF-8/JSON validation; the usage-injection branch serializes a new bounded body while the unchanged branch moves the exact original view; final scalar counting and downstream attempt-plan ownership are unchanged.

## Independent review
- Plan reviewer: `b0056_final_review`
- Code reviewer: `b0063_fresh_code_review`, distinct from the plan reviewer
- Plan verdict: Approved with the test constraint that pointer inequality is an implementation-level supplement, not an external wire contract
- Code verdict: Approved with no findings after a source-first review; allocation identity/semantics tests passed ten repeated runs.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline AiProxy tests | passed | 12 tests |
| Implementation and focused tests | passed | 14 AiProxy and 22 dispatch tests; five canonical ownership/semantics tests passed ten repeated runs in review |
| Gateway all-target compile | passed | implementation and review verification |
| Independent code review | passed | fresh reviewer distinct from the plan reviewer; no findings |
| Full campaign gates | passed | format, workspace check, workspace Clippy, agent docs, SSA, metrics inventory, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | existing integration tests unchanged from the user-authorized baseline |

## Handoff
- Ledger/changelog updated: yes; one changed and one examined-no-change
- Commit: `2cb73846`
