# Context

## Mandatory references

- `AGENTS.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/SKILL.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/references/campaign-artifacts.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/references/batch-workflow.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/references/subagent-contracts.md`
- `skills/SKILL.md`
- `skills/00-design-rules.md`
- `skills/01-architecture/SKILL.md`
- `skills/01-architecture/02-gateway/SKILL.md`
- `skills/03-coding/SKILL.md`
- `skills/04-review/SKILL.md`
- `skills/04-review/CLOSED-FINDINGS.md`
- `skills/05-testing/SKILL.md`
- `skills/07-tasks/SKILL.md`

## Progressive batch context

Load only the architecture, feature, coding, testing, and review documents that match the selected batch. The task entry records permanent guardrails; batch-specific context belongs in the subagent prompt and the changelog evidence, not in a growing copy of the Skills tree.

## Baseline and immutable integration-test floor

- Repository baseline: `d484e580`
- Baseline `edgion-tests` tree: `ae68dedd863020ddd6d035d7404570716429ecd1`.
- Allowed under `edgion-tests`: newly added files only.
- Forbidden under `edgion-tests`: modification, rename, or deletion of any baseline file.
- `integration-freeze.tsv` is the authoritative content-and-mode manifest for this boundary and is checked before delegation, after review, after verification, and immediately before staging.
