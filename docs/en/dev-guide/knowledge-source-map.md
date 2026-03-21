# Knowledge Source Map and Maintenance Rules

This document defines how `AGENTS.md`, `skills/`, `docs/`, and thin platform-specific wrappers should be used in the Edgion repository so the same knowledge does not drift across multiple places.

## Layering Rules

- `AGENTS.md`: shared cross-platform entry point for repository-wide rules, common commands, and navigation guidance.
- `skills/`: agent-facing task knowledge for high-frequency workflows, debugging paths, and project-specific constraints.
- `docs/`: human-facing long-form explanations, multilingual developer docs, design notes, and background material.
- Platform-specific files: thin wrappers only, such as `CLAUDE.md` and `.cursor/rules/00-edgion-entry.mdc`. They should not become a second knowledge base.

## Current Topic Map

| Topic | Canonical human-facing source | Agent-facing entry | Maintenance rule |
|------|-------------------------------|--------------------|------------------|
| AI collaboration | [ai-agent-collaboration.md](./ai-agent-collaboration.md) | [AGENTS.md](../../../AGENTS.md), [skills/SKILL.md](../../../skills/SKILL.md) | Keep collaboration flow in `AGENTS.md` and this guide instead of scattering platform behavior across many wrappers |
| Overall architecture | [architecture-overview.md](./architecture-overview.md) | [skills/01-architecture/SKILL.md](../../../skills/01-architecture/SKILL.md), [00-project-overview.md](../../../skills/01-architecture/00-common/00-project-overview.md) | `docs/` explains the big picture; `skills/` tells an agent what to read first |
| Resource architecture / registry | [resource-architecture-overview.md](./resource-architecture-overview.md), [resource-registry-guide.md](./resource-registry-guide.md) | [03-resource-system.md](../../../skills/01-architecture/00-common/03-resource-system.md) | Keep implementation-oriented resource mechanics primarily in `skills/`, with `docs/` staying human-friendly |
| Add a new resource type | [add-new-resource-guide.md](./add-new-resource-guide.md) | [00-guide.md](../../../skills/01-architecture/01-controller/09-add-new-resource/00-guide.md), [01-integration-testing.md](../../../skills/05-testing/01-integration-testing.md) | Keep executable workflow in `skills/`; keep broader background and examples in `docs/` |
| HTTP plugin development | [http-plugin-development.md](./http-plugin-development.md) | [12-edgion-plugin-dev.md](../../../skills/01-architecture/02-gateway/12-edgion-plugin-dev.md) | `docs/` explains execution stages and implementation boundaries, while `skills/` carries the concrete add-a-plugin workflow; end-user configuration remains in user-guide |
| Stream plugin development | [stream-plugin-development.md](./stream-plugin-development.md) | [13-stream-plugin-dev.md](../../../skills/01-architecture/02-gateway/13-stream-plugin-dev.md) | `docs/` explains implementation background and boundaries, while `skills/` carries the executable workflow; end-user configuration remains in user-guide |
| Runtime config / path behavior | [work-directory.md](./work-directory.md) | [SKILL.md](../../../skills/02-features/02-config/SKILL.md) | Keep path rules and process-level config selection in `skills/`; keep longer background explanation in `docs/` |
| Annotation mechanism | [annotations-guide.md](./annotations-guide.md) | [00-annotations-overview.md](../../../skills/02-features/10-annotations/00-annotations-overview.md) | `docs/` explains placement and implementation boundaries; detailed key tables and reserved-key lists live in `skills/reference` |
| Logging / observability | [logging-system.md](./logging-system.md) | [skills/03-coding/SKILL.md](../../../skills/03-coding/SKILL.md), [00-access-log.md](../../../skills/03-coding/observability/00-access-log.md), [02-tracing-and-logging.md](../../../skills/03-coding/observability/02-tracing-and-logging.md) | `docs/` covers system design; `skills/` covers implementation checks and operating rules |
| CI / release / image publishing | [ci-release-workflow.md](./ci-release-workflow.md) | [skills/09-misc/SKILL.md](../../../skills/09-misc/SKILL.md), [02-github-workflow.md](../../../skills/09-misc/02-github-workflow.md) | `docs/` explains release flow and manual review points; `skills/` carries the concrete workflow and commands |
| Work directory | [work-directory.md](./work-directory.md) | No dedicated skill yet; read the doc directly when needed | If this becomes a high-frequency workflow later, extract a skill; until then keep it docs-first |
| JWT Auth design note | [jwt-auth-plugin-design.md](./jwt-auth-plugin-design.md) | Read on demand only when the task is related | Design-review records stay in `docs/`; do not force them into a generic skill |
| Requeue / cross-resource dependency resolution | No standalone doc entry yet | [06-requeue-mechanism.md](../../../skills/01-architecture/01-controller/06-requeue-mechanism.md), [skills/04-review/SKILL.md](../../../skills/04-review/SKILL.md) | This is agent-first knowledge and should primarily live in `skills/` |

## Command Ownership

- Put repository-wide entry commands in [AGENTS.md](../../../AGENTS.md).
- Put workflow-specific commands in the matching `skills/*` document.
- Use `docs/` to explain when and why humans should run them, not to maintain a second giant command catalog.

Example:

- `cargo build`, `cargo test`, and the integration-test entry command live in `AGENTS.md`
- integration-test structure and debugging details live in [skills/05-testing/01-integration-testing.md](../../../skills/05-testing/01-integration-testing.md)
- human guidance for collaborating with AI lives in [ai-agent-collaboration.md](./ai-agent-collaboration.md)

## Maintenance Rules

1. When a shared fact changes, update the canonical source first and link to it elsewhere.
2. Do not duplicate long implementation details in both `docs/` and `skills/`.
3. When adding a new high-frequency workflow, add or improve a skill first.
4. When adding long background material, write it in `docs/` first and link from the skill.
5. Platform-specific files should remain thin entry wrappers, not grow into a second knowledge tree.
6. After changing `AGENTS.md`, `skills/`, or this dev-guide entry layer, run `make check-agent-docs` to catch broken links and stale architecture paths.

## Current Migration Priorities

Suggested next steps:

1. If more resource patterns appear later, add a new reference example only when the pattern repeats instead of growing the main workflow.
2. Decide whether `work-directory` should become a dedicated skill based on actual usage frequency; otherwise keep it in `docs/`.
3. If `logging` keeps growing, move detailed rule lists and examples into skill/reference material instead of expanding the navigation layer.
