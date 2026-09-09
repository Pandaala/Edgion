# B0238 — AI Guard Spans

## Scope

- Pipeline state: analysis-in-progress; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `5b050b0d`.
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
