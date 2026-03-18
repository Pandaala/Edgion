# Edgion Agent Guide

This file is the cross-platform, repository-level instruction entry for coding agents.
Treat it as the canonical shared layer for Codex, Cursor, Claude, and other agent tools.

## Start Here

- When a task needs project-specific context, read `skills/SKILL.md` first.
- Use progressive disclosure: root `skills/SKILL.md` -> relevant domain `SKILL.md` -> specific reference file.
- Do not load the whole `docs/` tree by default. Use `docs/` for human-facing narrative and multilingual material, and `skills/` for task workflows and project-specific operational knowledge.
- If the task is unclear, prefer the smallest relevant skill subtree instead of loading broad architecture material.

## Knowledge Map

- Architecture: `skills/architecture/SKILL.md`
- Development workflows: `skills/development/SKILL.md`
- Observability: `skills/observability/SKILL.md`
- Testing and debugging: `skills/testing/SKILL.md`
- Build and CI/CD: `skills/cicd/SKILL.md`
- Coding standards: `skills/coding-standards/SKILL.md`
- Review heuristics: `skills/review/SKILL.md`
- Task tracking: `skills/task/SKILL.md`
- Gateway API compatibility notes: `skills/gateway-api/SKILL.md`

## Common Workflows

- New feature that needs architecture context:
  1. Read `skills/SKILL.md`
  2. Read `skills/architecture/SKILL.md`
  3. Load only the directly relevant architecture files
  4. Then read `skills/development/SKILL.md`
  5. Finish with `skills/testing/SKILL.md` for validation

- Add a new resource type:
  1. `skills/development/00-add-new-resource.md`
  2. Choose the closest pattern reference from that workflow (`route-like`, `controller-only`, `plugin-like`, `cluster-scoped`)
  3. `skills/architecture/08-resource-system.md`
  4. `skills/architecture/01-config-center/SKILL.md`
  5. `skills/testing/00-integration-testing.md`

- Debug route, TLS, or sync issues:
  1. `skills/testing/SKILL.md`
  2. `skills/gateway-api/SKILL.md` when Gateway API semantics matter
  3. `skills/mis/debugging-tls-gateway.md` for TLS gateway routing issues

- Understand controller/gateway config and path behavior:
  1. `skills/development/04-config-reference.md`
  2. Load the matching reference file for controller, gateway, or `EdgionGatewayConfig`
  3. `docs/zh-CN/dev-guide/work-directory.md` when relative path behavior matters

- Understand `edgion.io/*` keys before changing manifests or docs:
  1. `skills/development/05-annotations-reference.md`
  2. Load the matching reference for `metadata.annotations`, `options`, or reserved/test-only keys
  3. Update stale examples instead of copying legacy keys forward

- Add or debug HTTP plugin behavior:
  1. `skills/development/01-edgion-plugin-dev.md`
  2. `skills/observability/00-access-log.md`
  3. `skills/testing/00-integration-testing.md`

- Add or debug stream plugin behavior:
  1. `skills/development/02-stream-plugin-dev.md`
  2. `skills/development/05-annotations-reference.md`
  3. `skills/testing/00-integration-testing.md`

- Change CI or release automation:
  1. `skills/cicd/02-github-workflow.md`
  2. `skills/cicd/00-local-build.md`
  3. `skills/cicd/01-docker-build.md`

## Common Commands

```bash
# Build
cargo build
cargo build --bin edgion-controller
cargo build --bin edgion-gateway

# Checks
cargo check --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test --all
make check-agent-docs

# Targeted integration tests
./examples/test/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>

# Full integration run
./examples/test/scripts/integration/run_integration.sh
```

## Knowledge Source Rules

- Keep `AGENTS.md` as the canonical cross-platform agent entry.
- Keep `skills/` as the canonical task-oriented knowledge layer.
- Keep `docs/` as the canonical human-facing documentation layer.
- Do not duplicate the same detailed content in both `skills/` and `docs/`; prefer one canonical source and link to it.
- If a tool needs a vendor-specific wrapper such as `CLAUDE.md` or `.cursor/rules/`, keep that wrapper thin and point it back to this file.

## Prompting Guidance For Humans

Humans do not need to paste large document lists into chat if the tool reads repository instructions.
Good prompts are short and task-shaped, for example:

- "Follow `AGENTS.md`. This feature needs architecture context before implementation."
- "Use the repo skills to understand the resource pipeline, then add the new resource."
- "Use the testing skill and Gateway API notes to debug this integration regression."

For more detailed collaboration patterns, see:

- `docs/zh-CN/dev-guide/ai-agent-collaboration.md`
- `docs/en/dev-guide/ai-agent-collaboration.md`
- `docs/zh-CN/dev-guide/knowledge-source-map.md`
- `docs/en/dev-guide/knowledge-source-map.md`
