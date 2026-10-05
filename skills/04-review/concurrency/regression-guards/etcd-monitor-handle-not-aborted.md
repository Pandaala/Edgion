---
name: etcd-monitor-handle-not-aborted
description: Etcd monitor cancellation ownership, lock-free RPC waits, and the limits of abort-only retirement and publication guarantees.
knowledge_type: regression-guards
---

# Etcd health-monitor cancellation and retirement boundaries

## Current conclusion

`EtcdLinkClient` in `edgion-common/src/link_sys_core/providers/etcd/client.rs`
stores its monitor `JoinHandle`. `mark_retired` deactivates health reporting,
sets `retired`, signals `shutdown_tx`, and takes and aborts the monitor handle.
`shutdown` calls that path before clearing the stored client under its write
lock and resetting local health/endpoint coverage. The monitor is not a
spawned-and-forgotten task.

The health probe clones the client inside a short read-lock scope and releases
the guard before awaiting `status()`. Reconnect and endpoint probes likewise
perform network IO without holding the client lock. Consequently a slow RPC
does not hold the read lock against shutdown. This is established by lock
scope, not by assuming that `abort()` synchronously releases a network-held lock.

`JoinHandle::abort()` requests cancellation; it does not wait for task exit.
An already-running poll can continue synchronous work until it yields or
completes. An `.await` whose future is immediately ready need not yield.
Neither an abort request nor dropping the handle proves that all task-local
resources have already been dropped when `shutdown()` returns. The current
implementation does not join the task and provides no exit acknowledgement.

## Retirement and publication guarantees

The following mechanisms have different scopes:

- The monitor checks `retired` around its waits and before attempting recovery
  publication. These checks stop work when retirement is observed; they are
  not a transaction with the later client write-lock acquisition.
- `PublishedHealth::deactivate` and `update` share a mutex in
  `edgion-common/src/link_sys_core/health.rs`. Deactivation is a real barrier for
  reporter callbacks: later updates cannot report for the retired generation.
  It does not prevent changes to that object's local health value.
- The persistent runner transfers health publication ownership with the
  current-client swap. The Gateway runtime table withdraws current lookup on
  removal. A retired object's internal client slot is not that table, and old
  caller-held handles have the provider's existing semantics.
- The reconnect branch checks `retired` before `client.write().await`, but does
  not recheck after acquiring the lock. The source therefore does not establish
  a strict no-write-after-retirement barrier for that slot. In particular, do
  not infer such a barrier solely from abort: a poll that passed the check can
  overlap shutdown and reach an immediately-ready lock acquisition. Endpoint
  coverage and local health writes also are not task-exit acknowledgements.

The accepted
[provider-owned removal contract](../../link-sys/intentional-design/provider-owned-runtime-and-simple-removal.md)
permits asynchronous retirement and old external handles; it does not claim
that all retired transport state is destroyed synchronously. Verify the actual
caller and publication boundary before classifying a late old-object update.
A requirement that shutdown finish all monitor work or prohibit every late
internal write needs separate synchronization or join evidence. This page must
not suppress a concrete violation of such a requirement.

## Historical evidence

Finding `link-sys-F012` was recorded as fixed on 2026-05-25 when the stored
monitor handle and abort backstop replaced a dropped spawn handle. The original
review described a read guard held across a health RPC and motivated abort as a
way to release that guard. That description is historical: the current probe
clones and unlocks before IO.

The earlier page also claimed deterministic completed termination, immediate
lock release, and exit confirmation from abort without joining. Those claims
were stronger than the implementation and are superseded by the boundaries
above. Retaining an abort handle addresses cancellation ownership, not exit
confirmation. Sibling Webhook abort-only decisions are lifecycle precedents,
not proof of completion for this monitor.

## Verification and re-evaluation

The 2026-09-27 source review inspected the monitor, retirement/shutdown paths,
`PublishedHealth`, and the current provider-removal contract. No new runtime
stress test or task-exit test was run for this documentation correction.
Historical test results do not establish present concurrency guarantees.

Re-evaluate when publication ordering, concurrent init/shutdown, monitor-handle
installation, lock scopes, or caller-held handle semantics change; when a
concrete late update affects current lookup; or when the task gains buffered
writes, leases, or other state requiring bounded cleanup. Keep the synchronous
handle mutex short and outside awaits. Do not add cross-provider teardown
barriers merely to imitate a provider with drainable buffers.

## Resolution references

- `link-sys-F012` — stored handle and abort backstop, 2026-05-25.
- [Webhook abort-only history](../intentional-design/webhook-active-health-abort-only.md).
- [Superseded Webhook registry coupling](../superseded/webhook-manager-rwlock-intentional.md).
