---
name: task
description: Use this skill when creating, updating, or continuing work under the repository's tasks directory. It defines how to structure task folders, step files, status tracking, and how to record analysis/design/implementation progress consistently.
---

# Task Skill

Use this skill whenever work needs to be recorded under `tasks/`.

> **Prerequisite:** Before creating any task, first read [skills/00-lifecycle/SKILL.md](../00-lifecycle/SKILL.md) to understand the full development lifecycle. This skill defines task **structure and conventions**; the lifecycle skill defines the **execution phases and gate criteria**.

## Goal

Keep task tracking consistent, lightweight, and incremental so future work can continue from the task files without reconstructing context from chat history.

## Directory Rules

- Use `tasks/working/` for active work.
- Use `tasks/todo/` for ideas or backlog items that are not yet active.
- Each active task should have its own folder under `tasks/working/`.
- Folder names should be short, stable, and kebab-case (e.g. `log-tracing-optimization`).

## File Structure

Each active task should usually contain:

1. A main task file named after the folder.
2. One file per step.

```text
tasks/working/<task-name>/<task-name>.md
tasks/working/<task-name>/step-01-<name>.md
tasks/working/<task-name>/step-02-<name>.md
```

## Main Task File

The main task file should contain:

- The task goal
- The current scope
- A flat list of steps
- Step status
- What is intentionally out of scope for the current phase

Keep it short and readable. It is the entry point, not the full design doc.

## Step Files

Each step file should have a single responsibility.

Recommended step sequence (aligned with [lifecycle phases](../00-lifecycle/SKILL.md)):

| Step | Lifecycle Phase | Purpose | Related Skills |
|------|----------------|---------|----------------|
| `step-01-*` | Phase 1: Analysis | 需求分析与影响评估 | [architecture/](../01-architecture/SKILL.md) |
| `step-02-*` | Phase 2: Design | 方案设计 | [architecture/](../01-architecture/SKILL.md), [development/](../02-development/SKILL.md) |
| `step-03-*` | Phase 3: Implementation | 编码实现 | [development/](../02-development/SKILL.md), [coding-standards/](../03-coding/SKILL.md) |
| `step-04-*` | Phase 4: Testing | 测试验证 | [testing/](../04-testing/SKILL.md) |
| `step-05-*` | Phase 5: Documentation | 文档更新 | [development/07-documentation-writing.md](../02-development/07-documentation-writing.md) |
| `step-06-*` | Phase 6: Review | 回顾与知识回写 | [review/](../07-review/SKILL.md) |

## Step Review Rule

Every step must explicitly check whether there are problems, risks, hidden assumptions, or unresolved design gaps.

Each step file should expose these clearly so they can be discussed and confirmed before moving on.

At minimum, each step should include one short section such as:

- `Current issues`
- `Risks`
- `Need confirmation`

Do not hide these in prose. Make them easy to scan.

## Status Conventions

Use simple lowercase status values:

- `pending`
- `completed`
- `blocked`

If only analysis/design is requested, mark later steps as future work and do not create implementation notes yet unless needed.

## Writing Guidance

- Prefer concise engineering prose over long narrative.
- Separate facts, decisions, and open questions.
- Be explicit about assumptions.
- Record why a direction is chosen, especially when it differs from historical architecture.
- When there is a phased plan, explain why the phases are split that way.

## Relationship to Skills

When task work also creates durable project knowledge:

- Put process rules in `skills/`
- Put task-specific conclusions in `tasks/`

Do not turn a task document into a general-purpose skill unless the workflow is reusable across future tasks.

## Post-Task Checklist

After completing a task, check whether any outputs should be propagated:

| Item | Target | When |
|------|--------|------|
| Reusable workflow patterns | `skills/` (appropriate category) | When the approach is generalizable |
| Review conclusions | `skills/07-review/` | When a review finding is project-specific and reusable |
| User-facing feature docs | `docs/en/`, `docs/zh-CN/` | When the change affects user-visible behavior |
| Architecture decisions | `skills/01-architecture/` | When the change alters system design |
| New resource/plugin docs | `docs/*/dev-guide/` | When a new resource type or plugin is added |

See [docs/DIRECTORY.md](../../docs/DIRECTORY.md) for the full user documentation structure.

## Default Workflow

When asked to create or continue a task:

1. Check whether the task already exists under `tasks/working/` or `tasks/todo/`.
2. Create or update the task folder.
3. Create or update the main task file.
4. Add or revise step files for the current phase.
5. Keep the task docs aligned with the actual scope of the current request.
6. In every step, explicitly surface problems and items that need discussion/confirmation.
