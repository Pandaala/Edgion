---
name: 07-tasks
description: Use this skill when creating, updating, or continuing tasks under the repository's tasks/ directory. Covers task structure, document set, lifecycle phases, status tracking, and task templates.
---

# Task management

Use this skill when work needs to be recorded under `tasks/`.

## Quick entry points

| Scenario | Direct entry |
|------|----------|
| End-to-end new-feature workflow (analysis → design → design review → implementation → code review → test reinforcement → full validation → summary) | [new_feature_work_flow.skill](new_feature_work_flow.skill) |
| Output templates for the new-feature workflow | [new_feature_work_flow_templates.md](new_feature_work_flow_templates.md) |
| Subagent contracts for the new-feature workflow | [new_feature_work_flow_subagents.md](new_feature_work_flow_subagents.md) |

## Directory rules

The `tasks/` layout has two independent axes:

**Lifecycle axis** — tracks active work:
- `tasks/todo/` — ideas / not yet started
- `tasks/pending/` — work in progress or pending start
- `tasks/block/` — work blocked on an external prerequisite; record the blocker and resume conditions near the front of the main task file
- `tasks/long-term-todo/` — deferred product bets outside the current API Gateway / AI Gateway scope. Keep them here so they stay visible without looking like near-term work. Do not pick one up through `/fix-issue-cn` or ordinary feature implementation until the user explicitly authorizes it.
- Acceptance happens via a GitHub PR. There is no separate `done/` directory; completed work is archived by git history once the branch is merged.

**Review-batch axis** — global review passes:
- `tasks/review-<batch-or-slug>/` — one file per finding from a global review pass; consumed by `04-review/fix-review-issue.skill`; finding files are deleted as each issue is closed.

A batch may define independent file-existence closure in its local `AGENTS.md`.
For such a batch, existing finding files are the open-task set. After required
workflow gates pass, deletion closes the selected finding without updating shared
INDEX, PLAN, STATE, coverage, counts or closure receipts. Shared review artifacts
retain historical provenance; do not restore deleted tasks from them. This local
policy overrides the default per-finding index maintenance requirement.

Task granularity is flexible: a single file `<task-name>.md` or a separate directory works. Use kebab-case for names (e.g., `log-tracing-optimization`); for complex tasks (multi-document set), use a directory.

## Task profiles

Tasks use four supported shapes. Pick the closest profile when creating a new
task; the prescribed-document set below applies only to `feature` tasks. Names
in the Shape column are illustrative unless the path is explicitly identified
as a current task.

| Profile | Required files | Optional files | When to use | Shape |
|---------|----------------|----------------|-------------|---------|
| `feature` | `<name>.md` (Meta + scope) | Full 7-doc set: `00-context.md` ... `07-changelog.md` | Multi-module changes, new features, design needed | `tasks/todo/<feature-name>/` |
| `investigation` | `README.md` | `01-...07-` numbered reports; optional `issues/` pool | Pure research, competitor analysis, scoping work | `tasks/todo/<investigation-name>/` |
| `review-batch` | `INDEX.md` | One file per finding: `<area>-NN-<slug>.md` | Output of a global review pass; consumed by `04-review/fix-review-issue.skill` | `tasks/review-<batch-or-slug>/` |
| `docs` / `single-file` | `<name>.md` | none | Small docs alignment, single-feature TODO, hotfix tracking | `tasks/todo/core-api-group-alias-contract.md` |

The phase pruning table below applies primarily to `feature` profile. For `investigation`, skip phases 4 (Test) and 5 (Review) — the deliverable is the report, not code. For `review-batch`, the entire batch is itself a workflow output; no per-finding phases.

## Task document set

> The document set below describes the **feature** profile's recommended files. Other profiles use fewer documents — see Task profiles above.


Each task directory contains the following documents, created on demand (only the main task file is required):

```text
tasks/pending/<task-name>/
├── <task-name>.md              # [required] Main task file: meta, requirements, scope, AI guide summary
├── 00-context.md               # [recommended] Related Skills index: list of skill paths used by this task
├── 01-design.md                # [recommended] Design doc: architecture, interfaces, data flow
├── 02-implementation.md        # [recommended] Implementation and tests: code plan, test strategy, coverage assessment
├── 03-subtasks.md              # [as needed] Subtask breakdown and progress tracking
├── 04-todo.md                  # [as needed] To-do items: scattered open items
├── 05-issues.md                # [as needed] Key issues: blockers, critical issues that can't be solved immediately
├── 06-decisions.md             # [as needed] Decision log: important decisions and reasons
└── 07-changelog.md             # [as needed] Changelog: key milestones and change records
```

| File | Purpose | When to create | Notes |
|------|------|----------|------|
| `<task-name>.md` | Task entry, meta + AI guide summary | Required when task starts | AI reads this file first when picking up the task |
| `00-context.md` | List skill paths used by the task so AI can locate quickly during coding | Before entering the implementation phase | Recommended; ensures AI has paths to follow |
| `01-design.md` | Architecture, interface design, data flow, config changes | Design phase | Technical blueprint for large tasks |
| `02-implementation.md` | Code plan + test strategy + coverage assessment (merged because they are tightly coupled) | Implementation phase | Recommended |
| `03-subtasks.md` | Subtask breakdown + per-subtask status | When changes exceed 3 modules or 5 files | Includes status, branch, output fields |
| `04-todo.md` | Scattered open items that don't form a separate subtask | As needed | Difference from `03-subtasks.md`: no clear deliverable |
| `05-issues.md` | Blockers / critical issues that can't be solved right now | Create as soon as a blocker is found | As needed; includes severity, workaround, and required fields |
| `06-decisions.md` | Important technical decisions and trade-offs | When the main file overflows with decisions | As needed |
| `07-changelog.md` | Key milestones | Long-running tasks | Not needed for short tasks |

## Main task file template

```markdown
# <Task title>

## Meta

| Key | Value |
|-----|-------|
| Created | YYYY-MM-DD |
| Status | pending / in-progress / completed / blocked |
| Type | feature / bugfix / refactor / docs / config |
| Priority | P0 / P1 / P2 / P3 |
| Issue | #xxx or N/A |

## AI guide summary

> One paragraph stating: what this task does, why, and the core difficulty.
> AI reads this paragraph first to understand the whole task.

## Requirements

(Original requirements, verbatim or summarized)

## Scope

### In scope
- ...

### Out of scope
- ...

## Document index

| Document | Status | Notes |
|------|------|------|
| 00-context.md | done | Related skills index |
| 01-design.md | done | Overall design |
| 02-implementation.md | in-progress | Implementation and tests |
| 03-subtasks.md | in-progress | Subtask progress |
| 04-todo.md | — | Not created |
| 05-issues.md | warning | 1 unresolved |
| 06-decisions.md | — | Not created |
| 07-changelog.md | — | Not created |

## Affected modules

| Module | Impact |
|--------|--------|

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
```

## Lifecycle phases

| Phase | Purpose | Skills to load | Documents produced |
|-------|---------|-------------|----------|
| 1 Analysis | Understand requirements, identify affected modules | `01-architecture/SKILL.md` | `00-context.md` |
| 2 Design | Define interfaces, data flow, config changes | `01-architecture/`, `02-features/SKILL.md` | `01-design.md` |
| 3 Implementation | Code per the design | `01-architecture/` (dev guide), `02-features/` (config schema), `03-coding/SKILL.md` | `02-implementation.md`, `03-subtasks.md` |
| 4 Test | Verify correctness | `05-testing/SKILL.md` | Update test coverage in `02-implementation.md` |
| 5 Review | Code review | `04-review/SKILL.md` | `06-decisions.md` |

### Phase pruning

| Task type | Required phases | May skip |
|-----------|----------------|----------|
| New feature | All | — |
| Bug fix | 1, 3, 4 | 2 (if the fix is obvious), 5 |
| Refactor | 1, 2, 3, 4, 5 | New behavioral tests may be unnecessary, but compile/lint and relevant regression verification are still required |
| Docs only | 3, 4 | 1, 2, 5; verification means relevant link, structure, and documentation checks rather than Rust tests |
| Config change | 1, 3, 4 | 2, 5 |
| Investigation-only | 1, 2 | 3 (Implementation), 4 (Test) ✗, 5 (Review) ✗ — deliverable is the report |
| Hotfix | 1, 3, 4, 5 | 2 may collapse into 1 if the fix is self-contained |

## Document creation rules

1. **Create on demand**: only create documents for the current phase; do not pre-create empty files
2. **Main file first**: every task has at least the main task file `<task-name>.md`
3. **Context first**: before entering the implementation phase, `00-context.md` should be ready so AI has paths to follow
4. **Subtask threshold**: when changes are estimated to exceed 3 modules or 5 files, create `03-subtasks.md` to break them down
5. **Record issues immediately**: write blockers into `05-issues.md` as soon as they appear; don't defer
6. **Write task content in English**: `tasks/` is not an exception to the repository's English-only rule; do not copy the language of legacy task files

## Status values

- `pending` — not started
- `in-progress` — in progress
- `completed` — completed
- `blocked` — waiting for a dependency
- `skipped` — skipped (must state the reason)

## Implementation-step checkpoints

- [ ] Follows `skills/03-coding/00-logging-and-tracing-ids.md`
- [ ] Follows `skills/03-coding/01-log-safety.md`
- [ ] `cargo fmt --all` has been applied
- [ ] `cargo check --workspace --all-targets` passes
- [ ] `cargo clippy --workspace --all-targets` is warning-free
- [ ] `python3 cicd/checks/validate_agent_docs.py` passes
- [ ] `python3 cicd/checks/check_ssa_force.py` passes
- [ ] Targeted tests appropriate to the change pass; run `cargo test --all` only when the user explicitly requests it

## Task-completion checklist

| Output | Target location | When |
|--------|--------|------|
| Reusable patterns | `skills/` | Solution is generally applicable |
| Review findings | `skills/04-review/` | Project-specific reusable findings |
| User docs | `docs/en/`, `docs/zh-CN/` | User-visible behavior change |
| Architecture decision | `skills/01-architecture/` | System-design change |
| New resource/plugin usage docs | `docs/*/user-guide/` or `docs/*/ops-guide/` | User-visible resource or plugin added |
| Contributor extension guide | `docs/*/dev-guide/` | Contributor-facing implementation workflow changed |

## Default workflow

1. Check whether the task already exists under `tasks/pending/` or `tasks/todo/`
2. Create the task directory or single file and write the main task file
3. Create `00-context.md` listing related skill paths
4. For new-feature tasks, prefer [new_feature_work_flow.skill](new_feature_work_flow.skill) as the execution template
5. Advance through the lifecycle phases, creating documents on demand
6. Write blockers into `05-issues.md` as soon as they appear
7. Update the document index and status in the main task file as work progresses
8. Hand off a review-ready change and ask for explicit authorization before any commit, push, or GitHub PR creation; merged work is archived in git history
