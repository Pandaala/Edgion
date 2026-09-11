# B0301 — HTTP Plugin Store

- Pipeline state: in-progress; current-HEAD freshness and frozen integrations passed before plan-first promotion.
- Active anchor: `edgion-gateway`; implementation baseline `6b34abd2` after current-HEAD refresh.
- Target: `edgion-gateway/src/plugins/http/plugin_store.rs`, reopened after cross-namespace mirror LKG and later sync changes; B0062/B0121 history is evidence to revalidate.
- Analyst must audit store publication, generation/lifecycle ownership, dependency resolution and LKG behavior across plugin compilation, conf-sync and request consumers, including all relevant external changes and tests.
- Existing `edgion-tests/**`, user-owned protected paths, campaign records and all technical files are read-only during analysis. No `fix-issue-cn`, commit or push.

## Analysis result

- Fresh child analyst `b0301_plugin_store_analysis` audited the complete target, compilation/full-delta publication, Controller policy stamps, mirror metadata, request consumers, external diffs, tests, B0062/B0121 and D0030/D0095 at `50d3b3ba`.
- Confirmed D0095: `CompiledPlugins::can_retain_previous` ignores executable Controller-stamped PluginPolicy actions. An incoming invalid DSL/WAF generation with a new Block stamp can retain the previous unblocked Arc in both `apply_delta` and `replace_all`; Bypass, removal or policy changes can likewise be discarded.
- Decision: change, complex/plan-first because this alters security-policy precedence in immutable publication. Exact technical allow-list is only `edgion-gateway/src/plugins/http/plugin_store.rs`, including colocated deterministic tests.
- Conservative requirement: disable whole-runtime LKG whenever either previous or incoming generation has an enabled stamped PluginPolicy action in any HTTP stage. Preserve ordinary no-policy LKG, mirror guard, add/update ordering, atomic snapshot/AccessLogExtern publication, Arc-pinned readers, seal ordering, D0030 boundary and all observability/redaction rules.
- Tests must cover DSL/WAF, delta/full paths, Block/Bypass/removal/recovery, all four stages, ordinary/inert no-policy retention and mirror behavior without network. No integration issue is needed.
- The narrow HTTP stamped-policy/LKG defect is owned here. The concurrently expanded canonical todo and D0095 remain open for the broader EdgionGatewayConfig policy migration; do not delete or duplicate them.

## Approved implementation plan

- External `79b72bb3` added RequestId scope wiring to the target; a fresh analyst revalidated D0095 and the one-file value case at that HEAD. Later `6b34abd2` did not touch the target or its policy/RequestId chain.
- Implementation subagent `b0301_implement` will add a private four-stage enabled-policy predicate, use it symmetrically for previous and incoming generations, and update the local LKG contract comments.
- Colocated deterministic tests will cover DSL/WAF, delta/full publication, Block/Bypass/removal/action/status changes, initial invalid and recovery, all stages, inert stamps, ordinary LKG, mirror interaction, Arc identity, fixed diagnostics, and RequestId phase composition.
- Plan reviewer `b0301_plan_review` required explicit RequestId preservation, mirror-bypass expectation correction, separated invalid/stamped fixtures, and contract comments; the revised plan was approved with no remaining Must Fix at `6b34abd2`.
- Exact technical allow-list remains `edgion-gateway/src/plugins/http/plugin_store.rs`; campaign records, D0095 mitigation records and the integration manifest are coordinator-owned commit support.

## Implementation

- `CompiledPlugins::can_retain_previous` now rejects whole-runtime LKG when either generation contains an enabled Controller policy stamp in any HTTP stage, while retaining the ordinary invalid DSL/WAF and mirror boundaries.
- Deterministic colocated tests cover delta/full publication, DSL/WAF, Block/Bypass/removal/action/status changes, initial invalid and recovery, all stages, disabled/no-policy behavior, mirror interaction, RequestId phase composition, Arc identity, and fixed bounded diagnostics.
- A pre-existing inline named-jump expectation was reconciled with the externally aligned frozen fixtures and maintained D0083 contract; all fixture bytes remain unchanged.

## Independent review

- Final reviewer `b0301_final_review` found no technical Must Fix and confirmed the change is cold-path-only, atomic, lock-free for readers, behavior-preserving outside policy precedence, and proportionate to the enforcement risk.
- Diagnostic `b0301_test_diagnostic` attributed the initial named-jump failure to committed fixture/test drift; an approved plan deviation corrected only the stale inline assertion.
- Fresh adjudicator `b0301_todo_adjudicate` resolved the concurrent-task disagreement: B0301 mitigates only the narrow HTTP LKG defect; the broader canonical migration todo and D0095 must remain open.

## Verification

- Focused policy tests passed; complete `compile_plugins_tests` passed 36/36 after the approved stale-test reconciliation.
- Formatting, locked workspace check, locked workspace Clippy (existing warnings, exit 0), agent-doc, SSA, metrics, Pingora source, Gateway tracing, English-only, frozen-integration and diff gates passed at `6b34abd2`.
- Frozen integrations remain unchanged by the campaign; external `79b72bb3` paths were verified and reconciled in the manifest.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic local commit after all gates passed.
