# Edgion Gateway Quality Campaign Handoff

> Resume with `$edgion-gateway-quality-campaign` in continue mode. Do not create another campaign, do not invoke `fix-issue-cn`, and never push.

## Current state

| Key | Value |
|-----|-------|
| Repository | `/Volumes/ExtStore/ws5/Edgion` |
| Branch | `feature-08-30` |
| Campaign root | `tasks/pending/edgion-gateway-file-optimization/` |
| Active anchor | `edgion-gateway/` |
| Processed files | 445 |
| Ignored files | 28 |
| Remaining files | 78 |
| Active batch | None |
| Running analysis | None |
| Integration freeze | 3,274 existing `edgion-tests/**` paths |

Execution was intentionally paused by the user on 2026-09-11. No campaign-owned analysis, implementation, test, or Cargo process is running.

## Simplified bookkeeping

Only `03-subtasks.md` owns live file status. It has three flat path sections:

- processed files;
- ignored files;
- remaining files.

After a batch commits, move its target paths from remaining to processed and update the counts. Do not add batch IDs, commit hashes, issue IDs, outcome notes, or task links to these lists.

`07-changelog.md`, `batches/`, and external synchronization records are append-only historical evidence. They never need updates when a standalone follow-up task is completed or deleted.

The former campaign-local deferred mirror and shared index were removed. Twenty-six still-active mirror-only findings were migrated into self-contained `tasks/todo/` files before removal; existing standalone tasks were not duplicated. A task must not depend on this campaign, refer back to a batch or deferred ID, or require any campaign update. Another agent may complete and delete the task file directly. No Markdown file currently links to a specific `tasks/todo/` file.

## Fixed scope

The 78 paths under `Remaining files` are the complete future target set. The 28 ignored paths cover pending AI-owned targets and standalone `edgion-gateway/tests/**` targets. Newly created or discovered Gateway files are outside the campaign unless the user explicitly changes the remaining list.

Analysis may read surrounding code and knowledge. Optimization must start from a remaining target. Any supporting edit outside that list requires direct correctness evidence and an exact batch allow-list; it does not become another coverage target.

## Resume point

Start a fresh read-only analysis from these four remaining plugin-runtime targets, then let the analyst decide whether they form one cohesive batch or should be narrowed:

- `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-gateway/src/plugins/runtime/traits/session.rs`

They were affected or reopened by RequestId/request-trace-id migration `79b72bb3`. A prior provisional B0310 analysis was interrupted before producing a report; do not reuse it.

## Required controls

- Read the repository rules and campaign skill before resuming.
- Verify the three file-list counts and the frozen integration manifest.
- Keep technical analysis, implementation, and independent review in separated agent roles.
- Keep exactly one mutation/review/verification/commit lane.
- Preserve unrelated concurrent work; HEAD movement alone is not a blocker.
- Never broadly stage, reset user work, edit existing `edgion-tests/**`, or push.
- Run `cargo test --all` only when explicitly requested.

The campaign simplification is intentionally record-oriented: file status is centralized, historical records are immutable, and standalone tasks have no shared lifecycle state.

At handoff, the frozen manifest check detects externally committed changes under `edgion-tests/**`. The simplification did not modify those files. Reconcile only verified external paths on resume; never reset or rewrite them to satisfy the manifest.
