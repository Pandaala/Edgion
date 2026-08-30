---
name: integration-report-must-cover-the-whole-run
description: Use when reviewing integration report ordering, completeness, or exit-status consistency.
---

# Integration reports must cover the whole run

The main integration `report.log` is authoritative only when it covers isolated preflight
scenarios, ordinary suites, and every mandatory final gate. Results produced before the shared
stack creates its work directory are buffered and included when the canonical report is
initialized.

During execution the report status is `INCOMPLETE`. The runner creates a pending report before its
first preflight and moves it into the shared work directory once that directory exists. Normal
termination finalizes it exactly once as `PASS` or `FAIL` after the final metrics, unknown-field,
requeue-loop, and cache-residue checks. A fail-fast or unexpected early exit leaves the report
`INCOMPLETE` and appends a `Run_Aborted` entry with the current phase and exit code. Report updates
use a same-directory temporary file plus rename so readers never consume a partially written
summary.

A process exit code and the final report status must agree. Explicitly skipped checks are recorded
as `SKIP` with a reason; an unrecorded check is not equivalent to a pass.
