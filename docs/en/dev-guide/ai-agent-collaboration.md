# AI Collaboration and Skill Usage Guide

This document explains how AI tools should use `AGENTS.md`, `skills/`, `docs/`, and repository commands in Edgion, without requiring humans to paste long document lists into every chat.

## Core Principles

- `AGENTS.md` is the shared cross-platform entry point.
- `skills/` is the task-oriented knowledge layer, not a mirror of the entire developer documentation tree.
- `docs/` remains the human-facing documentation layer for narrative explanations, contribution docs, and multilingual content.
- Prefer letting the agent navigate the repository knowledge structure on its own, instead of manually enumerating many files in chat.

## How To Ask AI To Use Skills On New Work

Recommended prompt shape:

```text
Please follow AGENTS.md and use skills/SKILL.md to find the relevant repo knowledge before analyzing or implementing this task.
This feature needs architecture context first.
```

More specific form:

```text
Please start with AGENTS.md, then navigate from skills/SKILL.md into the relevant architecture and development skills.
Only load the files that are directly relevant to this task.
```

For testing or debugging:

```text
Please use the testing skill and any relevant Gateway API or TLS debugging notes.
```

This works better than manually attaching many documents because it delegates knowledge discovery to the agent as part of the workflow.

## Should There Be One Big Skill

No. A single giant `SKILL.md` is usually the wrong shape.

The better pattern is a three-layer structure:

1. Repository entry: `AGENTS.md`
2. Root navigation: `skills/SKILL.md`
3. Domain navigation and task-focused skills: `skills/<domain>/SKILL.md` plus targeted reference files

Why this works better:

- Large single files create context bloat.
- Small scoped skills trigger more accurately.
- Domain navigation avoids repeating the same project background in every task file.

## Do We Need To Pass The Full Skill Navigation In Every Chat

Usually no, as long as the tool reads repository instruction files.

The preferred setup is:

- maintain a strong repository root `AGENTS.md`
- have `AGENTS.md` tell the agent to begin with `skills/SKILL.md`
- only add a manual reminder such as "please follow AGENTS.md" when a platform does not automatically read repo instructions

The real goal is not a special chat command. The real goal is a stable repository navigation entry.

## Is There A Universal Command

There is no single slash command that works across all platforms.

The cross-platform mechanism is instead:

- repository root `AGENTS.md`
- structured `skills/`
- thin platform wrappers when needed, such as `CLAUDE.md` and `.cursor/rules/00-edgion-entry.mdc`

You can still standardize short human prompts such as:

- `Please follow AGENTS.md`
- `Please navigate from skills/SKILL.md to the relevant skill`
- `This task needs architecture context before implementation`

That is close enough to a universal command in practice and is far more portable.

## Where Commands Should Live

Use a layered approach:

- `AGENTS.md`: only common repository-level commands
- relevant `skills/`: workflow-specific commands
- `docs/`: human-facing explanation of how to use those commands and how to collaborate with AI

For example:

- `cargo build`, `cargo test`, and the main integration test entrypoint belong in `AGENTS.md`
- workflow-specific testing detail belongs in `skills/05-testing/`
- human guidance for AI collaboration belongs in this document

Avoid maintaining the same command explanation in both `docs/` and `skills/`.

## How Commands And Knowledge Stay Compatible

Compatibility comes from treating commands as part of workflows, not as an isolated parallel knowledge system.

Recommended rule:

- workflows live in `skills/`
- shared commands live in `AGENTS.md`
- long-form explanations live in `docs/`
- each command should have one canonical home

Example for integration testing:

- the repo-wide entry command lives in `AGENTS.md`
- suite details and debugging stay in `skills/05-testing/00-integration-testing.md`
- human guidance for using the workflow with AI stays in this document

## Recommended Usage Pattern In This Repository

- let the agent read `AGENTS.md`
- let it navigate from `skills/SKILL.md`
- only load deeper files when the task actually needs them
- explicitly point to a document only when you already know it is critical

## Maintenance Guidance

- When you add a high-frequency workflow, prefer creating or improving a skill instead of writing a disconnected document first.
- When you add long-form explanation, prefer `docs/`, then link it from the relevant skill.
- If a tool needs a vendor-specific rules file, keep it thin and avoid letting it become a second knowledge base.
