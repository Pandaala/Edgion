---
name: integration-suite-lifecycle-errors-must-fail
description: Use when reviewing integration TestSuite hooks, empty selections, or test-client exit status.
knowledge_type: regression-guards
---

# Integration suite lifecycle errors must fail

Suite setup and teardown are assertions about whether a test ran in a valid environment. Preserve
their errors in `SuiteResult`, include them in console and machine-readable reports, and make them
affect the process exit status. Printing a hook error is not sufficient.

A test selection that executes zero cases fails by default because it usually signals registry or
dispatch drift. An intentionally empty probe must opt in through the runner's explicit
`allow_empty_selection` API. Zero executed cases have a zero percent pass rate, never 100 percent.

## Resolution references

- `ITEST-009` — 2026-08-26.
