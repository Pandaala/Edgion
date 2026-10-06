---
name: etcd-monitor-handle-not-aborted
description: Etcd monitor cancellation ownership and provider publication fencing without task-exit acknowledgement.
knowledge_type: regression-guards
---

# Etcd health-monitor cancellation and retirement boundaries

## Current conclusion

Common `EtcdLinkClient` retains its monitor JoinHandle and uses the upstream
Etcd SDK. Retirement locks provider-owned state, marks the generation retired,
deactivates PublishedHealth, signals shutdown and takes and aborts the monitor.
The same retirement mutex guards initial/recovery client-slot publication and
monitor installation. A candidate cannot publish after retirement merely because
it passed an earlier check. This fence is local and requires no SDK patch.

Network waits clone clients outside provider locks. The client slot write lock
is acquired before the short synchronous retirement guard; no retirement guard
is held across an await. PublishedHealth separately linearizes its reporter
callbacks, and the persistent runner owns current lookup/generation handover.

Aborting requests cancellation; it does not join the monitor or acknowledge task
exit. SDK transport state or caller-held streams may outlive provider withdrawal.
There is no connection-generation cancellation API or synchronous revocation
promise. Local health/coverage changes are not proof of completed destruction.

## Historical evidence

Finding link-sys-F012 was fixed on 2026-05-25 by retaining the monitor handle and
adding an abort backstop. The 2026-09-27 review corrected overclaims that abort
proved completed termination. B0084 subsequently used a patched SDK's
AdmissionControl. On 2026-10-06 the user withdrew that SDK feature set; provider
publication fencing remains, while SDK-specific cancellation claims are inactive.

## Verification and re-evaluation

Current focused tests include retirement before initialization and monitor
installation. Current verification is recorded in the B0084 task; historical
SDK election/cancellation tests do not prove upstream transport behavior.
Re-evaluate lock ordering, concurrent init/shutdown, monitor installation or
caller-held handles. A requirement for joined termination needs explicit evidence.

## Resolution references

- link-sys-F012: stored handle and abort backstop, 2026-05-25.
- B0084 / define-etcd-linksys-unused-config-fields: scope withdrawn on 2026-10-06.
