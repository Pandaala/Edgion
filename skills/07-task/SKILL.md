---
name: task
description: Use this skill when creating, updating, or continuing work under the repository's tasks directory. Covers task structure, step files, lifecycle phases, status tracking, and the task template.
---

# Task Skill

Use this skill whenever work needs to be recorded under `tasks/`.

## Directory Rules

- `tasks/working/` — active work
- `tasks/todo/` — backlog / ideas
- Each active task gets its own folder, kebab-case name (e.g. `log-tracing-optimization`)

## Task File Structure

```text
tasks/working/<task-name>/
├── <task-name>.md            # main task file (entry point)
├── step-01-analysis.md
├── step-02-design.md
├── step-03-implementation.md
├── step-04-testing.md
├── step-05-documentation.md
└── step-06-review.md
```

Sub-step variants: `step-03-implementation-types.md`, `step-03-implementation-handler.md`, etc.

## Main Task File Template

```markdown
# <Task Title>

## Meta

| Key | Value |
|-----|-------|
| Created | YYYY-MM-DD |
| Status | pending / in-progress / completed / blocked |
| Type | feature / bugfix / refactor / docs / config |
| Priority | P0 / P1 / P2 / P3 |
| Issue | #xxx or N/A |

## Requirement

(Original requirement, verbatim or summarized)

## Scope

### In scope
- ...

### Out of scope
- ...

## Lifecycle Phases

| Phase | Step File | Status | Note |
|-------|-----------|--------|------|
| 1 Analysis | step-01-analysis.md | pending | |
| 2 Design | step-02-design.md | pending | |
| 3 Implementation | step-03-implementation.md | pending | |
| 4 Testing | step-04-testing.md | pending | |
| 5 Documentation | step-05-documentation.md | pending | |
| 6 Review | step-06-review.md | pending | |

(Mark skipped phases as `skipped` with reason in Note)

## Affected Modules

| Module | Impact |
|--------|--------|

## Decision Log

| Date | Decision | Reason |
|------|----------|--------|
```

## Lifecycle Phases — What Each Step Does

| Phase | Purpose | Load Skills |
|-------|---------|-------------|
| 1 Analysis | Understand requirements, identify affected modules | `01-architecture/SKILL.md` |
| 2 Design | Define interfaces, data flow, config changes | `01-architecture/`, `02-features/SKILL.md` |
| 3 Implementation | Write code per design | `01-architecture/`（dev guides）, `02-features/`（config Schema）, `03-coding/SKILL.md` |
| 4 Review | Code review | `04-review/SKILL.md` |
| 5 Testing | Verify correctness | `05-testing/SKILL.md` |

### Phase Tailoring

| Task Type | Required Phases | May Skip |
|-----------|----------------|----------|
| New feature | All | — |
| Bug fix | 1, 3, 4 | 2 (if obvious), 5 (if no user impact), 6 |
| Refactor | 1, 2, 3, 4 | 5 (if no external change), 6 |
| Docs only | 5 | 1-4, 6 |
| Config change | 1, 3, 4 | 2, 6 |

## Step File Guidelines

Each step file should have a single responsibility and must include:

- **Facts/decisions** — what was decided and why
- **Risks / Open questions** — make them scannable, not buried in prose

### Implementation Step Checkpoints

- [ ] Follows `skills/03-coding/00-logging-and-tracing-ids.md`
- [ ] Follows `skills/03-coding/01-log-safety.md`
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets` no warnings

## Status Values

- `pending` — not started
- `in-progress` — active work
- `completed` — done
- `blocked` — waiting on something

## Post-Task Checklist

After completing a task, propagate outputs:

| Output | Target | When |
|--------|--------|------|
| Reusable patterns | `skills/` | Generalizable approach |
| Review conclusions | `skills/04-review/` | Project-specific, reusable finding |
| User-facing docs | `docs/en/`, `docs/zh-CN/` | User-visible behavior change |
| Architecture decisions | `skills/01-architecture/` | System design change |
| New resource/plugin docs | `docs/*/dev-guide/` | New resource or plugin |

## Default Workflow

1. Check if task exists under `tasks/working/` or `tasks/todo/`
2. Create/update task folder and main task file using the template above
3. Create/update step files for the current phase
4. Surface problems and open questions explicitly in every step
5. Update phase status in main task file as work progresses
