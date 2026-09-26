# Campaign Context

## Workflow

- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/SKILL.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/references/campaign-artifacts.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/references/batch-workflow.md`
- `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/references/subagent-contracts.md`

## Repository knowledge entry points

- `AGENTS.md`
- `skills/SKILL.md`
- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/SKILL.md`
- `skills/04-review/SKILL.md`
- `skills/04-review/knowledge-routing.md`
- `skills/05-testing/SKILL.md`
- `skills/07-tasks/SKILL.md`

Each technical agent must start from these indexes and progressively load only the specific architecture, coding, review, and testing material relevant to its batch.


## Common phase knowledge routing

Coverage planner: `common_coverage`, baseline `18e03b4c65b5225d0b299db55b8fb17c65232df5`.

- `skills/01-architecture/00-common/SKILL.md`
- `skills/01-architecture/00-common/02-core-layout.md`
- `skills/01-architecture/00-common/05-unification-points.md`
- `skills/01-architecture/03-controller-gateway-link/SKILL.md`
- `skills/03-coding/SKILL.md`
- Topic indexes under `skills/04-review/`: link-sys, conf-sync, observability, ssrf, tls.
- `skills/05-testing/00-unit-testing.md`

Load only the relevant topic for each batch and verify its original rules and current consumers. Shared code still follows the calling binary's logging ownership and canonical request observability. No Common auth module exists; federation protocol generation is Controller-owned.
