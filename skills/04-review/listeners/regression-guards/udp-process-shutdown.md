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

Normal cancellation lets an in-progress expiry log/removal pair finish before the
remaining-session pass. Forced abort can abandon unsubmitted terminal logs and
in-flight datagrams. Log submission uses the existing best-effort logger; it does
not promise persistence or sink flush. The
[DataSender shutdown task](../../../../tasks/todo/data-sender-graceful-shutdown-contract.md)
owns that independent contract. Serving-time expiry/activity and same-key receiver
generation races remain in
[session retirement](../../../../tasks/todo/retire-udp-sessions-by-generation.md).

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

Registry tests cover multi-listener cancellation, duplicate/late registration,
repeated/concurrent shutdown, failed task reporting, and parent-abort/child-
destruction acknowledgement without network I/O. The existing serve cancellation
test also checks session and permit cleanup.

Real signal/UDP-wire verification remains explicitly deferred in
[UDP process shutdown wire coverage](../../../../tasks/todo/udp-process-shutdown-wire-coverage.md).
Re-evaluate if shutdown timing moves to initial signal receipt, listeners become
dynamic, task ownership changes, a blocking operation enters these futures, or
the session retirement / shared sender lifecycle contracts change.

## Resolution references

- `wire-udp-listener-process-shutdown`: confirmed defect, fixed and verified;
  the active source task was removed. The explicitly deferred wire coverage,
  session-generation retirement, and sender persistence work remain tracked above.
