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

## Coverage teardown must preserve the Gateway profile

The ordinary integration cleanup keeps a five-second TERM-to-KILL window, but
coverage mode gives only the Gateway a bounded 60-second window. The instrumented
Gateway otherwise reaches Pingora's default 30-second grace period and is killed
before normal process exit can flush its LLVM raw profile. Controller, test-server,
and non-coverage teardown deadlines remain unchanged. Do not change production
shutdown defaults or broaden process matching to accommodate the test harness.

`integration-coverage-teardown-grace` reproduced the boundary on macOS arm64 on
2026-09-14. With the old five-second cleanup, Gateway PID 35212 was escalated to
KILL and produced zero matching profiles. Allowing the same instrumented binary
to exit normally took about 35 seconds and produced one mergeable 1,710,832-byte
profile. After the fix, two consecutive real main-runner executions of
`HTTPRoute/Match` completed successfully: Gateway PIDs 52436 and 54404 each
produced one 1,710,832-byte profile, and both profiles merged successfully with
`llvm-profdata merge -sparse`. The focused regression command is
`bash cicd/checks/tests/integration_coverage_teardown_test.sh`; it checks the
coverage-only deadline selection and distinguishes a completed exit hook from a
child killed before that hook completes. Re-evaluate if Pingora's default shutdown
window, the coverage profile runtime, or integration process ownership changes.

## Evidence boundary: back-to-back local runs

`integration-suite-back-to-back-run-collapse` recorded a historical macOS
symptom in which most cases immediately failed to connect on some consecutive
local runs. Its proposed mechanism is not a valid lifecycle rule: the test
cleanup waits five seconds before escalating the Gateway from TERM to KILL,
while the Gateway defaults to a 30-second Pingora grace period followed by a
10-second graceful-shutdown timeout. Escalation at five seconds therefore does
not show that the Gateway ignored TERM. Nor does a terminated listening socket
remain bound merely because established TCP connections can enter TIME_WAIT.

Current startup independently kills stale test binaries, waits, checks the
shared stack ports, and then requires the Gateway process, admin and probe
listeners, cache readiness, and resource convergence before executing cases.
On 2026-09-10, three immediate back-to-back
`HTTPRoute/Transport` runs passed after each preceding cleanup escalated the
Gateway to KILL. No bind failure or target-listener connection refusal appeared.
This disproved the recorded causal claim and did not reproduce the historical
failure rate; it does not prove that all platforms or future runner revisions
are immune to lifecycle failures. This section limits the evidence that can
support the lifecycle rule above; it is not a separate assertion that the
runner can never collapse.

If the collapse returns, preserve both runs' teardown and startup logs and
identify the failed bind, owning PID, and listener before proposing a fix. A
`SIGKILL after 5s` line alone is not evidence of port retention. Continue to
fail closed on an actual startup, readiness, or listener-contract failure.

## Resolution references

- `ITEST-009` — 2026-08-26.
- `integration-suite-back-to-back-run-collapse` — closed as stale/incorrectly
  attributed on 2026-09-10; no runtime or runner change.
- `edgionplugins-resource-sweep-config-list-drift` — full-suite follow-up: OTLP shutdown delivery assertions.
- `integration-coverage-teardown-grace` — coverage-only Gateway shutdown window.
