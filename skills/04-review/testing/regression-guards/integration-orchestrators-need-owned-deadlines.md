---
name: integration-orchestrators-need-owned-deadlines
description: Use when reviewing shell test schedulers, child waits, or integration timeouts.
knowledge_type: regression-guards
---

# Integration orchestrators need owned deadlines

Readiness polling and per-test timeouts do not bound a complete integration job. Every scheduler
that waits for child scripts must enforce a configurable deadline around the whole child lifecycle,
including setup, Docker operations, test execution, and teardown. Aggregate runners also need a
whole-run budget so a sequence of individually bounded jobs cannot exceed the intended run window.

Use `utils/run_with_deadline.py` for these boundaries. It starts the command in a new session, sends
TERM to that owned process group at the deadline, waits a bounded grace period, then sends KILL.
Callers treat exit 124 as `TIMEOUT`, preserve the child log, and continue to their final summary.
Do not replace state-based readiness checks with sleeps. Deadline wrappers and schedulers must
signal only the process group they created, never processes selected by global name. The canonical
fixed-port local shared topology is a separate, explicitly accepted single-run exception whose
startup and teardown clear its known integration processes; see the
[single-run cleanup decision](../intentional-design/local-shared-topology-single-owner-takeover.md).

## Process-group probes and zombie members

`ITEST-DEADLINE-EPERM` exposed a real timeout-classification defect on macOS:
after TERM, the unreaped leader was a zombie, `killpg(pgid, 0)` raised EPERM,
and the wrapper exited 1 instead of 124. A direct lifecycle experiment observed
successful probing while running, EPERM in state Z, and ESRCH after `wait()`.
The [XNU implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
filters zombies in `killpg1` and can return EPERM when no eligible member remains.
This is not evidence that arbitrary permission failures mean a group is gone.

The shared helper now disambiguates EPERM from probes and TERM/KILL through
`group_has_live_members`: a successful `ps -axo pgid=,stat=` snapshot must show
no non-zombie member of the owned PGID before the error is suppressed. A live
member preserves the permission failure; failed, timed-out, or malformed
snapshots fail visibly. The snapshot command has a one-second timeout. Other
groups never become signal targets. The leader remains unreaped until the final
`wait()`, retaining its PID while cleanup uses it as the group identity.

The grace period still bounds escalation; an exceptional snapshot adds bounded
query overhead. A leader's exit alone is not proof that descendants exited.
Processes that deliberately leave the owned session/group are outside this
helper's ownership boundary. Environments hiding group members from `ps` are
outside the verified platform assumptions; reassess before relying on this
fallback in such an environment.

## Verification and re-evaluation

Focused regression command:
`PYTHONDONTWRITEBYTECODE=1 python3 cicd/checks/tests/run_with_deadline_test.py`.
On 2026-09-09, all seven tests passed on macOS arm64 with Python 3.9 and a Linux
arm64 container with Python 3.11 and procps: normal
and reserved child exits, timeout exit 124, TERM-resistant child, surviving
descendant after leader exit, unreaped leader, and injected permission/snapshot
failure boundaries, including an empty snapshot. The minimal Linux fixture
initially lacked `ps`; after installing procps in the disposable container the
complete suite passed. Independent macOS review also exercised real SIGTERM
and SIGINT forwarding, yielding 143 and 130 without tracebacks.
Repository checks passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`
(warnings in unchanged Rust files), and the agent-doc, SSA-force,
metrics-inventory, and Pingora source-policy guards. No full integration sweep
or `cargo test --all` was run for this focused helper change.
The original ITEST-006 entry below remains historical evidence.

Re-evaluate when changing session ownership, leader reaping, signal forwarding,
process visibility, supported `ps` implementations, or timeout exit mapping.

## Direct OpenSSL operations inside the test client

ARCH-288 identified a separate boundary inside the Rust client: synchronous
OpenSSL waits in LKG generation/serial helpers and SNI collision, fallback-SNI
and single-label wildcard suites prevented cooperative case and phase timeouts
from running. An outer job watchdog did not make those local waits cooperative
or let a captured transition error reach LKG restoration.

These selected helpers now delegate direct-child ownership to private
`edgion-tests/src/client/tls_child.rs`. A detached supervisor keeps Child while
concurrently writing stdin, reading both output pipes and waiting, under one
absolute operation deadline. Caller receiver closure cancels the exchange but
does not cancel the supervisor's termination and explicit wait. Failure is sent
after the kill attempt and before wait finishes; bounded failure decisions and
cleanup completion are distinct. Successful/nonzero Output is returned only
after normal explicit wait and both output streams complete.

Discovery and its handshake share ten seconds across candidates; LKG served
certificate extraction shares ten seconds across s_client/x509. Standalone
fixture extraction and rotation generation each have ten seconds. Missing
candidates can be ignored; other spawn/IO errors and deadlines fail. In
particular, timeout is neither expected handshake rejection nor a LibreSSL skip.
The suites retain their own candidate policy, parsing and domain assertions.

Network-free tests include concurrent saturated input/output, genuine nonzero
output, blocked input/completion deadlines, input/output caller cancellation and
test-only acknowledgments of actual explicit wait completion. Discovery tests
exercise missing, unsupported, pending and permission-denied candidates. LKG
tests cover real fixture serial extraction, A/B rotation, initial seeding failure
before mutation, and captured failure/restoration error combination. The terminal
helper test proves restoration invocation, not real Controller Secret restoration.
Live handshake/status/restore evidence requires the existing shared integration
topology; these controlled-child tests make no network assertion.

Run `cargo test --locked -p edgion-tests --lib tls_child::tests` and the relevant
suite/scenario module test filters. Fresh run results are recorded in the scoped
implementation record; this description does not claim live integration or
cross-platform execution. Re-evaluate on owner cancellation/IO changes, runtime
lifetime changes, new child consumers or changed discovery semantics.

This is direct-child ownership, not a general subprocess-tree framework. OS
uninterruptible state and runtime shutdown/panic prevent absolute cleanup-time
guarantees; kill-on-drop is only a shutdown fallback. Arbitrary cancellation of
the enclosing scenario does not promise async Secret restoration. Cipher's
existing watchdog and the outer Python process-group helper remain separate.

## Resolution references

- `ITEST-006` — 2026-08-26.
- `ITEST-DEADLINE-EPERM` — 2026-09-09; corrected macOS zombie-group handling.
- `ARCH-288` — selected Rust OpenSSL helpers now own deadline/cancellation cleanup.
