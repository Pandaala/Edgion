---
name: etcd-configuration-and-message-boundaries
description: Review the upstream Etcd SDK boundary, supported configuration and removed options.
knowledge_type: regression-guards
---

# Etcd configuration and upstream SDK boundary

## Conclusion and scope

On 2026-10-06 the user withdrew the custom SDK feature set to avoid maintaining
third-party code. Etcd uses crates.io `etcd-client` 0.18 with its TLS feature.
There is no repository-local SDK, custom gRPC message envelope guard,
connection-bound version/cluster admission, member discovery or RPC observer.

Supported configuration is endpoints, Secret-backed authentication, canonical
outbound TLS, dial/request timeout, HTTP/2 keepalive and namespace. Resources
owns pure endpoint, timeout and keepalive validation; Common construction and
Gateway fingerprints validate the same fields. Admin validation reuses
`LinkSys::validate_config`. TLS verification remains enabled and resolved through
`resolve_outbound_tls`; dedicated election connections omit unary request timeout.

The former `autoSyncInterval`, `maxCallSendSize`, `maxCallRecvSize`, `userAgent`,
`rejectOldCluster`, `minClusterVersion`, `observability` and `timeout.keepAlive`
fields are removed, not accepted as supported no-ops. The canonical schema walk
reports them as unknown, and typed serialization does not retain them.

## Rationale and limits

B0084 found operator fields with no consumers. Its first implementation added a
local SDK and extensive transport machinery. That approach and its SDK-specific
CI, measurements and acceptance matrix are superseded by the user's scope change.
Historical SDK test counts do not describe the upstream dependency now selected.

Normal KV, Watch, Lease, Lock and election operations use upstream SDK semantics.
There is no Edgion-configurable all-service pre-encoding or pre-buffer message cap,
and no server-version or cluster-identity policy. DNS, dialing and transparent
connection recovery are SDK-owned, without Edgion dial-time egress classification.
This accepted existing-connector boundary assumes trusted operator-configured
endpoints; it does not waive the canonical egress rule for new paths. Revisit if
the upstream SDK exposes the required hooks without a maintained fork.

The provider retains bounded health probes, reconnect backoff, binary-owned
published-health transitions, monitor cancellation and generation publication
fencing. Removing the SDK does not imply synchronous termination of caller-held
Watch/Lease/election handles. See the monitor cancellation knowledge page.

## Verification and re-evaluation

Source: B0084 / `tasks/todo/define-etcd-linksys-unused-config-fields.md` and the
2026-10-06 user instruction. Current checks are recorded in that task; earlier
local-SDK verification is historical only. Re-evaluate dependency changes,
supported fields, unknown-field diagnostics, TLS/timeout mapping and retirement.
Do not restore a local SDK or the withdrawn features without explicit scope.
