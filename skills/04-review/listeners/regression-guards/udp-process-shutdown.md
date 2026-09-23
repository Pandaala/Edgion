---
name: udp-process-shutdown
description: Review UDP process shutdown signal delivery, task ownership, completion, and the auxiliary drain boundary.
knowledge_type: regression-guards
---

# UDP process shutdown and task completion

## Conclusion and scope

The Gateway cancels UDP listeners after Pingora returns, at the start of the
auxiliary-service drain. UDP keeps serving during Pingora's own shutdown grace.
The registry closes registration monotonically, owns each listener token and serve
handle, and serializes repeated shutdown calls. Duplicate or late registration is
rejected before spawning. The physical port set stays frozen at startup.

`EdgionUdpProxy::serve` owns cleanup and upstream-receiver handles in a local
`JoinSet`, reaps finished children while serving, and cancels children when the
parent exits. Each child is also wrapped by `TaskTracker::track_future`, allowing
the registry to await its destruction even when dropping an aborted parent's
`JoinSet` initiates child cancellation. These are process/task ownership records,
not a new resource cache or a dynamically configurable store.

Normal shutdown joins cleanup and receivers before logging remaining sessions.
At that point no task can change their counters or retire them concurrently.
Session permits and sockets are released through the existing session owners.
Traffic observability stays in the UDP access logger; no per-session tracing or
additional metrics are introduced.

## Rationale and evidence

Source finding `wire-udp-listener-process-shutdown` identified real missing wiring:
`add_udp_listener` registered cancellation tokens, but neither registry cancellation
method had a production caller. Serve, cleanup, and receiver handles were discarded.
The auxiliary watch signal did not reach UDP, so only runtime teardown stopped it.
No resource leak after process exit was claimed.

Current ownership spans:

- `edgion-gateway/src/runtime/server/listener_builder.rs`: creates the listener,
  cancellation token and child tracker, then registers and spawns atomically.
- `edgion-gateway/src/routes/udp/mod.rs`: closes registration, cancels all tokens,
  joins listeners under one absolute deadline, and acknowledges aborted children.
- `edgion-gateway/src/routes/udp/edgion_udp.rs`: owns and reaps children, makes
  downstream sends cancellable, and finalizes sessions after producer completion.
- `edgion-gateway/src/cli/mod.rs`: cancels UDP after Pingora, keeps its runtime alive
  during drain, and also drains on partially completed listener-construction failure.

The authoritative lifecycle is documented in
[Gateway startup and shutdown](../../../01-architecture/02-gateway/01-startup-shutdown.md).
The [frozen port-set boundary](../compatibility-constraints/listener-port-set-frozen-at-startup.md)
remains applicable; it did not justify the former missing process cancellation.

## Alternatives and limits

The five-second budget is shared with auxiliary-service grace, not restarted per
listener. On timeout all remaining parents are aborted before waiting for any of
them; their children are acknowledged separately. Tokio cancellation is cooperative:
acknowledgement can exceed the grace deadline, so this is not a hard process-exit
bound. Timeout or a failed task is a shutdown failure and yields a non-zero exit.
The CLI directly awaits the shutdown operation without an outer cancelling timeout.

Normal cancellation lets in-progress retirement and log submission finish before the
remaining-session pass. Forced abort can abandon unsubmitted terminal logs and
in-flight datagrams. Log submission uses the existing best-effort logger; it does
not promise persistence or sink flush. The
[process-exit log delivery decision](../../link-sys/intentional-design/process-exit-log-delivery-best-effort.md)
records that independent accepted limitation. Serving-time expiry/activity and
same-key receiver generation handling are covered by
[session retirement](udp-session-generation-retirement.md).

## Verification and re-evaluation

Current verification:

- `cargo test -p edgion-gateway --lib routes::udp::`: 38 passed, 0 failed.
- `cargo check -p edgion-gateway --all-targets`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with warnings; this is not a
  claim that the workspace is warning-free.
- `cargo fmt --all` applied; `cargo fmt --all -- --check`: passed.
- Agent-doc, gateway no-tracing, SSA force, metrics inventory, and Pingora source
  policy guards: passed.
- Two separate read-only code-review subagents: LGTM in both rounds. A prior plan
  review identified the partial-listener startup-failure branch; it was included
  before the code reviews. Neither source review substitutes for executed checks.

Fresh process-level verification for follow-up `udp-process-shutdown-wire-coverage`:

- `bash edgion-tests/integration/scripts/integration/test_udp_process_shutdown.sh`:
  passed on 2026-09-13 on macOS.
- The isolated scenario starts a real Controller, Gateway, and streaming UDP
  fixture with two listeners. SIGTERM preserves UDP traffic during Pingora's
  configured grace period, while SIGINT and an inbound-PROXY-policy fatal
  transition preserve their fast/non-zero semantics as applicable. All running
  cases stop forwarding after the post-Pingora drain marker, exit only after the
  auxiliary drain, and release both ports while the process is still alive.
- A separate startup case reserves the second UDP port, observes the first
  listener start, and verifies the non-zero bind failure releases the earlier
  port before process completion. Running cases observe terminal records for
  both listeners in the configured local-file UDP sink before process completion.
- The scenario is registered in the canonical local and Kubernetes isolated-job
  inventories. This run exercised the canonical standalone script locally; it
  did not execute a Kubernetes Job.

Registry tests cover multi-listener cancellation, duplicate/late registration,
repeated/concurrent shutdown, failed task reporting, and parent-abort/child-
destruction acknowledgement without network I/O. The existing serve cancellation
test also checks session and permit cleanup.

Real signal and UDP-wire coverage now exercises normal cooperative process drain.
The forced-abort branch remains deterministic unit coverage: the wire scenario
does not add a production-only hang or test hook merely to force the five-second
deadline. Observing the local-file record proves that sink received the submitted
entry in this scenario; it does not establish a durable-flush guarantee for every
asynchronous sender. Re-evaluate if shutdown timing moves to initial signal
receipt, listeners become dynamic, task ownership changes, a blocking operation
enters these futures, or the session retirement / shared sender lifecycle
contracts change.

## Resolution references

- `wire-udp-listener-process-shutdown`: confirmed defect, fixed and verified;
  the active source task was removed.
- `udp-process-shutdown-wire-coverage`: confirmed test gap, closed with the
  process-level scenario described above. Session-generation retirement is
  covered by [its regression guard](udp-session-generation-retirement.md);
  sender persistence remains separate tracked work.
