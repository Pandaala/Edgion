# Edgion Gateway File Optimization

## Status

- Campaign state: paused
- Active anchor: `edgion-gateway/`
- Processed files: 445
- Ignored files: 28
- Remaining files: 78
- Active batch: none
- Running analysis: none
- Push policy: never push

## Live files

- `03-subtasks.md` is the only live file-status tracker. It contains the complete processed, ignored, and remaining path lists.
- `integration-freeze.tsv` protects existing `edgion-tests/**` files.

The manifest currently detects externally committed `edgion-tests/**` drift. Reconcile only verified external paths when execution resumes; do not reset or rewrite tests to satisfy the manifest.

## Historical records

- `07-changelog.md` is an append-only summary of completed work.
- `batches/` contains append-only completed batch records.
- `external-sync-*.md` and `batches/external-sync-*.md` are append-only synchronization records.

Historical records are evidence only. They do not own file status or follow-up task lifecycle, and they are not updated when a standalone task is completed or deleted.

## Scope

Only paths under `Remaining files` in `03-subtasks.md` may start a new optimization batch. AI-owned targets and standalone `edgion-gateway/tests/**` targets are under `Ignored files` and are not processed.

Analysis may read surrounding code and documentation. Any supporting edit outside a remaining target requires direct correctness evidence and an exact batch allow-list; it does not add another coverage target. Newly created or discovered Gateway files are outside this fixed campaign unless the user explicitly changes the remaining list.

## File-status updates

After a batch is committed, move each completed target from `Remaining files` to `Processed files` and update the three counts above and in `03-subtasks.md`. Do not add batch IDs, commit hashes, issue IDs, task links, or outcome details to the file lists.

## Standalone follow-up tasks

Every follow-up is a self-contained file under `tasks/todo/`. It must contain all evidence, scope, constraints, and acceptance criteria required by another agent without consulting this campaign.

Do not create campaign backlinks, deferred mirrors, shared indexes, D-number identifiers, or lifecycle references. When another agent completes a standalone task, it may delete that task file directly without editing this campaign or any historical batch record.

## Execution rules

- Resume with `$edgion-gateway-quality-campaign` in continue mode.
- Keep one serial implementation, review, verification, and commit lane.
- Use separated analysis, implementation, and independent-review agents as required by the campaign workflow.
- Existing `edgion-tests/**` files remain frozen.
- Preserve unrelated concurrent work and never broadly stage files.
- Commit completed batches locally in English under campaign authorization; never push.
- Do not invoke or recommend `fix-issue-cn` while this campaign is active.
