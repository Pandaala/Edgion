# B0238 — AI Guard Spans

## Scope

- Pipeline state: changed; verification and independent review complete, awaiting local commit.
- Active anchor: `edgion-gateway`.
- Baseline: `e0e6c1a3`; target blob `7d6f2645d8fee5fe27a63807e73fbc7b4813fde0`, plugin caller blob `a0e83c1b908487f7a8045058dcfda836aca213b2`.
- Target: `edgion-gateway/src/plugins/http/ai_guard/spans.rs`.
- Inspect complete bounded JSON span selection, decoded-string ownership, role/content semantics, byte accounting, token ranges, callers and tests. B0237 separately owns rule replacement construction.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after B0237 before promotion.

## Analysis Outcome

- Decision: change, complex/plan-first. The target blob was `7d6f2645d8fee5fe27a63807e73fbc7b4813fde0` at `5b050b0d`; refresh the target and caller after B0237 commits.
- The current early span limit treats encoded quoted-token length as an upper bound on decoded bytes. The implication is reversed: an escaped one-byte value such as `"\\u0061"` is rejected under a one-byte decoded limit. Use the JSON string worst-case expansion relation only for safe early rejection, then retain decoded length as authoritative.
- Duplicate `type` or `text` fields in a content part can make span selection ignore text that the later canonical JSON parse forwards. Match the existing duplicate `messages`/`role`/`content` fail-closed policy by rejecting ambiguous content-part discriminators and text.
- Check `maxSpans` before decoding the first excess span. Also bound decoding of fixed-vocabulary `role` and `type` discriminators from their maximum recognized decoded lengths, retaining escaped recognized values and current unknown-role/non-text semantics.
- Exact code scope is `spans.rs`; the existing AI Guard guide may clarify duplicate content-part rejection if the approved plan includes the behavior-contract correction. Do not change the shared parser, BodyStorage, rules/plugin/evaluator, resources, integration tests, logging or metrics.
- Required tests cover escaped exact/plus-one decoded bounds, escaped recognized role/type, both duplicate-field orders for `type` and `text`, early span-count rejection and all existing span/AI Guard behavior. Existing frozen integration coverage is sufficient; no new issue is needed.
- Freshness: the original analyst revalidated after B0237 at `e0e6c1a3`. B0237 changed only `rules.rs`; `RuleSpan`, `GuardDecision`, `CompiledRules::evaluate` and the plugin's selection/failure boundary remain compatible, so all four findings and the one-file production scope remain valid.
- Frozen plan candidate scope: `spans.rs` plus the existing AI Guard guide only if required to state the newly enforced duplicate content-part behavior. No shared parser, BodyStorage, rules/plugin/evaluator, resource/schema, logging, metric or integration edit. Implementer must return a no-edit plan for independent review.

## Outcome

- The first independent plan review caught that the shared `field` helper also reports non-object nodes as ambiguous. The approved revision first preserves scalar/null/array part skips, limits duplicate rejection to actual object fields, corrects the complete affected guide paragraph, and adds surrogate/non-text/mixed-array regressions.
- A fully saturating `decoded_limit * 6 + 2` predicate is now the safe early bound for quoted JSON tokens. It accepts escaped values that decode within the configured limit; decoded UTF-8 byte length remains authoritative, and the physical BodyStorage cap remains the fallback when arithmetic saturates.
- The first excess selected span is rejected before raw-bound checks or string decoding. Fixed-vocabulary role/type decoding is also prebounded by the maximum recognized decoded lengths, keeping temporary discriminator ownership constant-sized while preserving escaped, unknown and invalid-value behavior.
- Non-object content-array entries, missing/unsupported types and non-text parts retain their skip semantics. Duplicate object `type` fields and duplicate `text` fields on a uniquely text part now fail closed in either order, preventing selection from disagreeing with downstream serde JSON last-wins interpretation.
- The AI Guard guide now accurately distinguishes missing, non-string, duplicate and unsupported message/content-part behavior without implying that missing content/type or non-object parts are refused.
- Fifteen span tests cover escaped ASCII and surrogate boundaries, zero/saturated limits, vocabulary prebounds, duplicate fields in both orders, non-text and mixed-array exemptions, and count-before-decode precedence. Existing span ordering/role/content behavior remains covered.
- Fresh independent final reviewer `b0237_ai_guard_rules_analysis`: LGTM, no Must Fix or Should Fix. It independently checked the bound proof, offsets, decode ordering, duplicate semantics, compatibility exceptions, guide, tests and project hard rules.
- Verification: spans 15/15 and full AI Guard 49/49; workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed. Existing integration files were not edited.
- Local commit SHA is backfilled by the next batch; never push.
