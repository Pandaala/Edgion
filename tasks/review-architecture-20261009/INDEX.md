# Architecture review findings

## Open tasks

Existing ARCH finding Markdown files are the open-task set. After the applicable
workflow gates pass, delete only the selected finding file to close it. IMPL and
AIMPL closure must not edit this index, PLAN, STATE, coverage, counts or receipts.
See the local [closure policy](AGENTS.md).

List the current tasks from the repository root:

```bash
rg --files tasks/review-architecture-20261009 -g 'ARCH-*.md' | sort
```

This index deliberately contains no per-finding links, rows or persisted counts,
so independent agents can close different findings without a shared index edit.

## Historical review evidence

The [original review plan](PLAN.md), [coverage checkpoint](COVERAGE.md),
[review state](STATE.json), [baseline](BASELINE.json), partition reports and
verification receipts retain their original evidence and limitations. Their
finding statuses and paths are historical, not live task state. Missing finding
files after closure do not invalidate those snapshots or require synchronization.

Review evidence must still be revalidated against current source before acting
on a finding. File-based closure does not bypass required reviews or checks.
