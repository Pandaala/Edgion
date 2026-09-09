# B0236 — AI Guard Plugin

## Scope

- Pipeline state: changed; verification and independent review complete, awaiting local commit.
- Active anchor: `edgion-gateway`.
- Baseline: `3a4cc119`; refreshed after B0234/B0235.
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
- Freshness: analyst revalidated at `3a4cc119`. Target blob remains `5278722781a18a64a77e42c9158cc1d9f0feebda`; evaluator support is committed blob `675e95604fcc8cf863d4867a3cb823e85fce2df3`. `CompiledEvaluator::decide` still accepts owned String pairs, so all three candidates remain coherent.
- Frozen plan candidate scope: target, evaluator support, and `docs/en/user-guide/http-route/filters/edgion-plugins/ai-guard.md` to correct the directly audited `maxSpanBytes` contract (absent inherits resolved physical cap; configured cap maximum 8 MiB). No resource/schema/BodyStorage/integration edit. Implementer must return a no-edit plan for independent review.

## Outcome

- Independent plan reviewer required preserving body-unavailable as a direct refusal even with `failOpen`, proving that path never latches, verifying borrow source identity, and distinguishing explicit 8 MiB configuration ceilings from an inherited physical cap that may be larger. The amended plan was approved before implementation.
- The physical `u64` request-body cap now converts to `usize` with checked saturation on narrow targets. Error-mode refusals no longer parse the request JSON; ContentFilter alone derives model/stream at the refusal boundary. Dependency fail-open paths latch and return before parsing.
- `apply_local` and `CompiledEvaluator::decide` now carry borrowed span text. The evaluator still serializes an owned, capped `Bytes` request before permit or network awaits, and the scoped merged projection ends before the owned decision is reassigned.
- The user guide now states that omitted per-span and aggregate span bounds inherit the resolved physical request-body cap (32 MiB by default and potentially above 8 MiB), while 8 MiB is only the ceiling for explicitly configured values.
- Focused tests cover both body-unavailable rejection modes under `failOpen` with no decision latch, plus ordered pointer identity for unchanged and replaced span sources. Existing behavior tests remain intact.
- Fresh independent final reviewer `b0236_final_independent_review`: LGTM, no Must Fix or Should Fix. It independently checked refusal/failure semantics, borrow lifetime, cap portability, documentation, scope and all project hard rules.
- Verification: AI Guard 32/32; workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed. Existing integration files were not edited by the campaign.
- External user commit `6a5e428a` was independently proven unrelated to the three frozen B0236 paths; its sole changed integration file was reconciled exactly in [the provenance record](../external-sync-6a5e428a.md).
- Commit: `5b050b0d`; never pushed by the campaign.
