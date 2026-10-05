---
name: etcd-monitor-handle-not-aborted
description: Etcd monitor cancellation ownership and generation publication fencing without task-exit acknowledgement.
knowledge_type: regression-guards
---

# Etcd health-monitor cancellation and retirement boundaries

## Current conclusion

Common EtcdLinkClient owns a monitor AbortHandle. Retirement deactivates
operation observations and PublishedHealth, cancels AdmissionControl, signals
shutdown and aborts the monitor. Network waits use cloned clients outside the
provider lock. Aborting is cancellation ownership, not a synchronous task join.

## Retirement and publication guarantees

AdmissionControl::cancel and while_active share a mutex. Initialization,
recovery publication and monitor-handle installation execute inside while_active,
so cancellation fences later publication into the retired generation. Endpoint
admission shares this control, including historical and election connections.
This replaces the former check-retired-then-lock race; a late candidate cannot
publish merely because it passed an earlier atomic check.

PublishedHealth and the Gateway observation adapter separately linearize
deactivation against their callbacks. The persistent runner still owns current
lookup and generation handover. Caller-held handles do not keep canceled
physical connections alive, but accepted buffered messages may be delivered
before stream termination. New policy does not retroactively revalidate them.

AbortHandle::abort and connection cancellation request shutdown; no monitor join
or transport task-exit acknowledgement is promised. An already-running poll may
finish synchronous work before yielding. Do not equate the publication fence
with completed destruction of every task-local resource.

The [provider-owned removal contract](../../link-sys/intentional-design/provider-owned-runtime-and-simple-removal.md)
continues to permit asynchronous retirement without cross-provider join barriers.

## Historical evidence

Finding link-sys-F012 was fixed on 2026-05-25 by retaining a monitor handle and
adding an abort backstop. The 2026-09-27 review corrected earlier overclaims
that abort proved completed termination. That limitation remains valid.
Its description of a JoinHandle and the absence of a publication fence became
stale when B0084 introduced AbortHandle and shared AdmissionControl.

## Verification and re-evaluation

The B0084 stage review inspected Common publication/cancellation and SDK
connection ownership. SDK election topology/cancellation tests passed on
2026-10-05; Gateway tests cover retirement against completion callbacks.
Final acceptance remains in
`tasks/todo/etcd-linksys-config-support/03-subtasks.md`.

Re-evaluate changes to admission-control lock ordering, publication,
concurrent init/shutdown, monitor installation or retained stream semantics.
Keep the synchronous control lock outside awaits. A new requirement for joined
termination needs explicit completion evidence.

## Resolution references

- link-sys-F012: stored handle and abort backstop, 2026-05-25.
- B0084 / define-etcd-linksys-unused-config-fields: generation publication fence.
