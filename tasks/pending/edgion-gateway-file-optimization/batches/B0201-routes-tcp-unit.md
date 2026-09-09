# B0201 — TCP Route Unit

## Scope

- Pipeline state: terminal.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tcp/route_unit.rs`.
- Baseline: `3e3e3606`.
- Freshness confirmed at `9853a685`; B0199 retains the same selected unit generation after dropping the table. Fresh target tests 5/5.
- Analyst: `b0201_tcp_unit_analysis`.
- Investigate immutable schema ownership, compiled backend selection state,
  callers, configuration boundaries and shared mechanisms before the value gate.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Decision: examined-no-change. Target blob `c62f50d10f411d9bf80c1c6b2371fed0ddc81b48`; fresh target tests 5/5 passed.
- The unit retains the original immutable schema Arc and configuration-time selector. Unchanged listener rebuilds reuse the unit/WRR; changed generations create new units while old connections retain their chosen generation.
- First-rule selection, retained denied/unsupported weight, post-select dial gates, canonical keepalive parsing and dynamic stream-plugin lookup remain correct.
- Selector backend-reference clones express independent ownership; removing them requires a shared API change without a demonstrated value case. Timeout policy belongs to the listener GatewayConfig, not the unit.
- Mixed optional weights are rejected by an explicit shared all-or-none policy and test. No TCP-only normalization is proposed and no Gateway API conformance claim is inferred.
- Resource architecture's registration-order statement differs from the table's key sorting; route to the later table batch rather than treating it as a unit defect. No files modified.

## Independent Review and Verification

- Reviewer: `b0201_independent_review`; LGTM, no Must Fix or worthwhile Should Fix. Ownership/generation, first-rule selector, denied weights, dynamic plugin lookup and shared policy verified independently; no API conformance claim inferred.
- Fresh independent tests: route unit 5/5, TCP 22/22, backend selector 22/22.
- Workspace format, locked check/Clippy, agent-doc, SSA, metrics, Pingora, Gateway tracing, English-only, diff and frozen-path checks passed. No source or integration edits.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
