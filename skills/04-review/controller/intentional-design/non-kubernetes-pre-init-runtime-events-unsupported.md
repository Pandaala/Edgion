---
name: non-kubernetes-pre-init-runtime-events-unsupported
description: Use when reviewing FileSystem or etcd resource-controller findings about Apply/Delete events arriving before the initial InitDone event.
knowledge_type: intentional-design
---

# Non-Kubernetes runtime events start after initialization

## Conclusion and scope

The FileSystem and etcd controller lifecycle does not support ordinary runtime
`Apply` or `Delete` events before a resource kind receives `InitDone`. Both
backends complete the phased `Init` -> `InitApply`* -> `InitDone` dispatch in
`spawn_all_controllers` before their runtime watcher starts. The event-loop
branches that accept an `Apply` while `init_done == false` are defensive only;
their parse-failure behavior is not required to match the supported runtime
invalid-resource path.

This decision accepts that a directly injected or newly introduced unsupported
event sequence may log or ignore an invalid pre-init `Apply` without revoking a
previous generation or persisting `ParseError`. Supported initial documents use
`InitApply`, and supported runtime updates use the worker-owned invalid-envelope
path after `InitDone`.

## Rationale and evidence

`non_kube_common::spawn_all_controllers` primes the backend, launches each
resource controller, calls `dispatch_init` for each dependency phase, and waits
for provider and Gateway readiness before starting their dependent phases.
The final dependent phase dispatches initialization without awaiting every
kind's readiness. Each per-kind FIFO still places InitDone before runtime
events; global readiness of all dependent controllers is not required here. Both watcher implementations
make `dispatch_init` send `Init`, zero or more `InitApply` events, and `InitDone`.
Only after `spawn_all_controllers` returns do
`FileSystemController::run` and `EtcdController::run` call `run_runtime`, which
is the source of ordinary `Apply` and `Delete` events.

The supported invalid-input paths remain unchanged. A malformed initial
document is handled by the `InitApply` parse-error branch and can persist an
error status. A malformed runtime update stores a `RuntimeInvalidEnvelope`,
removes the raw object, and enqueues the authoritative key so the single worker
revokes derived state and writes the error status in order. Repeated identical
errors are deduplicated by `ErrorStatus::error_for_persist`.

## Alternatives and limits

The rejected improvement was to unify parse, revocation, status persistence,
and repair behavior for an ordinary `Apply` observed before `InitDone`, with
focused FileSystem and etcd tests. That would harden a sequence with no current
production source, while adding another initialization transition to maintain.

This is not evidence that arbitrary event injection is safe, and it does not
authorize starting either runtime watcher earlier. It also does not weaken the
one-workqueue/one-worker rule or the accepted inline status-IO ordering. If a
future lifecycle change permits runtime events to overlap initialization, the
pre-init branches must be re-evaluated before that change is accepted.

## Verification and re-evaluation

The source ordering was revalidated when closing the task: the common startup
orchestrator completes phased initialization dispatch before returning, and each backend
calls `run_runtime` only afterward. Independent review and repository checks for
the closure are reported in the task run rather than treated as permanent test
evidence here.

Re-evaluate when `spawn_all_controllers`, either backend controller's startup
order, watcher initialization/runtime separation, event injection APIs, or
compaction/relist event types change.

## Resolution references

- `non-k8s-pre-init-apply-invalid-status-parity` (created 2026-09-12): real
  defensive parity gap, explicitly declined as unnecessary for the supported
  lifecycle on 2026-09-14; closed without a code or test change.
