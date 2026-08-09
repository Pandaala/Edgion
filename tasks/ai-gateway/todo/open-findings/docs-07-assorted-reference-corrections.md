# docs-07 — Four factual reference errors in the staged docs

| Field | Value |
|-------|-------|
| ID | `docs-07` |
| Disposition | `confirmed` |
| Severity | P3 |
| Dimension | Docs / i18n |
| Location | see the table below |
| Reviewer | Observability / docs review |
| Status | open |

## Summary

Four independent one-line documentation errors, grouped here because each fix is a single edit. They are unrelated to each other beyond being doc-vs-reality mismatches.

## Details

| # | Location | Claim | Reality |
|---|----------|-------|---------|
| 1 | `tasks/archive/completed-tasks/ai-gateway-anthropic-translation/00-context.md:69` | Skills to load include `skills/06-documentation/SKILL.md` | That path does not exist. `skills/06-*` is `06-tracing/`; the documentation tree is `skills/11-doc/` — and this very change set edits `skills/11-doc/00-doc-structure.md`. |
| 2 | `skills/05-testing/references/integration-suite-map.md:13-15` | Adds rows for `SingleProvider`, `Resilience`, `CredentialExhaustion` | `EdgionAIService/AnthropicProvider` is fully wired in this same change set (conf directory, `AnthropicProviderTestSuite` export, runner registration in `test_client.rs`) but has no row. |
| 3 | `skills/01-architecture/05-resources/20-edgion-backend-traffic-policy.md:53-54` | Presents `Conflicted` in a sentence-series alongside `Accepted` and `ResolvedRefs`, reading as a third condition type | GEP-2648 folds conflict into `Accepted=False/Conflicted`. The handler's own module doc says "there is no separate `Conflicted` condition type", and the sibling feature doc gets it right. The staged rewrite correctly deleted the old standalone table row but carried the ambiguity into prose. |
| 4 | `docs/zh-CN/ops-guide/observability/metrics-inventory.md:5` | Links the English authority as `../../en/ops-guide/observability/metrics-inventory.md` | From `docs/zh-CN/ops-guide/observability/`, `../../` is `docs/zh-CN/`, so the link resolves to `docs/zh-CN/en/…` and does not exist. Needs `../../../en/…`. Verified on disk. This is a residual of the `docs-01` fix (closed 2026-08-04), not a new page defect. |

## Impact

1. An agent following the task context wastes a lookup, or silently skips the documentation-structure rules the change is meant to follow.
2. The suite map is the single lookup surface for "which suite covers X"; the Anthropic translation path — the largest new protocol surface in this change set — appears untested to anyone consulting it.
3. An operator or agent writes an alert or status assertion keyed on `.status.ancestors[].conditions[?(@.type=="Conflicted")]`, which never matches, so oldest-wins arbitration losses go undetected.

## Suggested fix

1. Change to `skills/11-doc/SKILL.md`.
2. Add the row: `` | `EdgionAIService/AnthropicProvider` | `edgion-tests/integration/conf/EdgionAIService/AnthropicProvider/` | `AnthropicProviderTestSuite` | Yes | Anthropic Messages translation, JSON and SSE incremental response translation | ``
3. Reword to: "… and an oldest-wins arbitration loss is reported as `Accepted=False` with reason `Conflicted` (GEP-2648; there is no separate `Conflicted` condition type)."
4. Change `../../en/…` to `../../../en/…`.
