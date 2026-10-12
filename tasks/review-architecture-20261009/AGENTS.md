# Independent finding closure

This batch uses file existence as its task lifecycle. This policy implements the
user's explicit request for independent closure by concurrent agents.

- Each existing `ARCH-<id>-<slug>.md` is an open finding. After the applicable
  workflow's evidence, review and verification gates pass, delete that exact file
  to close it. A missing file is no longer an open task; do not recreate it from
  historical records.
- IMPL, AIMPL, AIMPL-C, fix-issue-cn and deltask must use this closure policy.
  Closing the selected finding is authorized as part of handling it. Deletion
  does not replace the workflow's correctness or acceptance gates.
- Do not update `INDEX.md`, `PLAN.md`, `STATE.json`, `COVERAGE.md`, shared counts,
  partition reports or historical receipts when closing a finding. Do not add
  acceptance receipts or another shared closed-findings ledger solely for closure.
- Shared review artifacts are historical evidence, not the live task registry.
  Their recorded IDs, statuses, paths and hashes describe their original review
  checkpoints. They need not resolve to an existing finding after closure and
  cannot require an index/state synchronization step.
- Find remaining tasks by listing existing finding files. Do not regenerate an
  index table or persist a remaining-task count. The generic index must not link
  to individual finding files.
- Preserve historical evidence and reusable semantic knowledge. Substantive
  implementation may still update owning architecture/review knowledge when
  required, but deleting a finding alone requires no knowledge edit.
- Each agent deletes only its selected finding. It must preserve unrelated
  findings and shared artifacts; deletion of a whole batch is not authorized.
- `.tools/` contains historical review-writing helpers. Do not run them for task
  closure or use them to regenerate shared finding state or links.

These lifecycle rules override the older batch plan and skill defaults that
require updating a shared index/state or prohibit deleting a source finding.
Root production-code rules, review requirements and Git authorization still apply.
