---
name: etcd-health-probe-timeout-zero-request-gigo
description: Historical zero-timeout deferral, superseded by canonical Etcd validation.
knowledge_type: superseded
---

# Former Etcd zero-timeout deferral

## Current conclusion

Inactive. B0084 now rejects zero dial/request timeouts through canonical
`EtcdClientConfig::validation_errors`, invoked by Controller preparse, Admin
validation and Common runtime construction. Gateway fingerprinting also rejects
invalid policy. Do not apply the former accepted/deferred verdict to current code.
See [Etcd configuration boundaries](../../link-sys/regression-guards/etcd-configuration-and-message-boundaries.md).

## Historical evidence

On 2026-06-16, linksys-health-degrade task 02 deferred `timeout.request: 0`
because fixing only the health probe would leave ordinary RPC timeout behavior
inconsistent. The useful conclusion remains: validate policy centrally rather
than silently flooring one consumer. The current implementation supplies that
shared validation; the old instruction against re-reporting this input is removed.

## Verification and re-evaluation

Resources boundary tests cover zero timeouts. Final B0084 acceptance remains
tracked in `tasks/todo/etcd-linksys-config-support/03-subtasks.md`. Re-evaluate if
a new construction path bypasses canonical validation. This supersession does
not claim every source-mode integration test has passed.
