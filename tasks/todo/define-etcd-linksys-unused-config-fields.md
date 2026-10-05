# Resolve unconsumed Etcd LinkSys configuration fields

## Evidence and impact

Found during Common quality batch B0084. `EtcdClientConfig` in
`edgion-resources/src/resources/link_sys/etcd.rs` exposes autoSyncInterval,
maxCallSendSize, maxCallRecvSize, userAgent, rejectOldCluster, observability, and
timeout.keepAlive. `edgion-common/src/link_sys_core/providers/etcd/config_mapping.rs`
and runtime client do not consume these fields. Operators can set them without
the described behavior taking effect; message-size knobs cannot currently be
relied on as limits. This task does not assert an observed production incident.

## Decision

For each field, implement its documented behavior with locked etcd-client 0.18
capabilities, or reject/remove the unsupported field and update all schema/docs
and callers. Do not silently accept a no-op. Distinguish timeout.keepAlive from
the actively mapped top-level keepAlive time/timeout. Prefer rejection/removal
for unneeded unsupported features; no pre-release compatibility shim is required.

## Constraints and acceptance

Preserve authentication, shared TLS resolution, namespace semantics, Controller
conf-center defaults, and the separate election connection without unary request
timeout. Inventory all callers and user docs, decide per field, and add validation
and real transport tests for each claimed supported behavior. Include defaults,
illegal input, reload and source modes; Controller advisory status alone cannot
replace runtime enforcement. Reuse shared integration topology and canonical
metrics rather than adding unbounded labels. Gateway API routing semantics are
unaffected; resource serialization and unknown-field feedback must remain coherent.
