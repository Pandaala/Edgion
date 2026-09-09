# B0236 — AI Guard Plugin

## Scope

- Pipeline state: analysis-ready; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `2466e808`.
- Target: `edgion-gateway/src/plugins/http/ai_guard/plugin.rs`.
- Inspect the complete plugin implementation, body/cap/failure/observability ownership, compilation and runtime callers and tests. B0234 separately owns evaluator serialization; B0235 owns the facade.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after B0234 before promotion.

## Analysis Outcome

- Analyst `b0236_ai_guard_plugin_analysis` recommends change with plan-first complexity at `2466e808`; clean target blob `5278722781a18a64a77e42c9158cc1d9f0feebda`. Full plugin and cohesive module/body/config/runtime/test call chains were read.
- Confirmed target-local fixes: replace the resolved physical body-cap `u64 as usize` with the canonical saturating checked conversion for 32-bit correctness; delay full JSON request-facts parsing until a rejection/failure response actually needs model/stream fields, removing a tree parse/allocation on clean pass and mask paths.
- After B0234, a supporting evaluator edit can borrow strings returned by `apply_local` instead of cloning every unchanged selected span before the awaited outbound decision. This requires a lifetime-bearing async cross-file interface and must be refreshed against B0234's committed blob before planning.
- Candidate plan-first scope: target plus evaluator support only. Preserve canonical BodyStorage/physical cap, applicability and guard-before-AiProxy generation/latch ordering, local masking before egress, evaluator caps/deadline/client/egress/TLS, exact error/content-filter/fail-open behavior and restrained diagnostics. Existing 30 module tests are the verification floor; no integration task is needed.
- Confirmed separate documentation discrepancy: the AiGuard Bounds table's `maxSpanBytes` default/maximum conflicts with code/schema and its own earlier prose. Keep it out of B0234; after B0236 planning, either approve the exact doc as behavior-contract support or create a self-contained documentation task.
- Original root/design, plugin/body/config/outbound/failure/observability/Rust/testing and relevant request-body review knowledge were independently applied. Existing D0082 remains separate.
