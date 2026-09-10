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

## OTLP shutdown delivery

`run_otlp_test.sh` starts the Gateway before its collector, so early warmup
traffic can be shed until the endpoint probe recovers. A fixed total of ten
records is not a valid shutdown assertion. Once collector delivery is observed,
send five successful requests with distinct `x-trace-id` markers and require
all five unique markers after shutdown; unrelated or duplicate records cannot
hide a missing request. A failed request or failed process shutdown is fatal.

Wait for actual Gateway exit within a bounded deadline longer than its default
30-second Pingora grace period. Do not claim exit after an unconditional
15-second wait. Poll collector persistence separately. This proves delivery
across graceful shutdown, including periodic export during the grace period;
it does not prove a provider `Drop` hook ran (the sender is process-lifetime).

The first 2026-09-10 run reproduced 7 total records versus a hard-coded minimum
of 10 and a premature exit message. See the current
[OTLP endpoint-health evidence](../../link-sys/intentional-design/healthy-semantics-intentional-non-flips.md).

Verification on macOS arm64: `bash cicd/checks/tests/otlp_shutdown_records_test.sh`
passed the marker-selection boundaries, and the corrected OTLP job passed all
12 assertions via `run_all_integration.sh --no-prepare --full-test --only linksys_otlp`.
The other six standalone jobs passed in the preceding aggregate run. The main
job remained blocked by missing host loopback aliases; these results do not
establish a successful full main sweep.

## Resolution references

- `ITEST-009` — 2026-08-26.
- `edgionplugins-resource-sweep-config-list-drift` — full-suite follow-up: OTLP shutdown delivery assertions.
