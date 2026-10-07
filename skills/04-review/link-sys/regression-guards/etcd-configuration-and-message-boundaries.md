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

Source: B0084 / `define-etcd-linksys-unused-config-fields` and the 2026-10-06
user instruction. The user authorized task closure and deletion after verification.
The upstream-SDK removal passed workspace check, Clippy, format and product-binary
builds with `--locked --offline`; metadata and Cargo.lock resolve one registry
`etcd-client` 0.18.0. Focused Etcd tests passed in Common (29), Controller (34),
Gateway (8) and Resources (16), with ten existing real-service tests left ignored.
The real LinkSys suite passed 89/89; standalone Controller election/demotion and
Gateway leader failover/reload suites passed. SSA-force, metrics inventory,
Pingora-source policy, Gateway API transform, unit-test layout and Gateway tracing
guards passed. Original logs are `/tmp/edgion-etcd-removal-{check,clippy-final,tests,build,linksys,election,ha}.log`.

The unrelated WAF task-link blocker was resolved, and agent-doc validation passed
again at closure on 2026-10-06. The user's shared-topology report
(`tmp/integration_testing/testing_20261006_121044/report.log`) records 257 passed,
seven skipped, zero failed and zero timed out; this report does not replace the
separate Etcd real-service evidence above. Earlier local-SDK verification remains
historical only. Re-evaluate dependency changes,
supported fields, unknown-field diagnostics, TLS/timeout mapping and retirement.
Do not restore a local SDK or the withdrawn features without explicit scope.

## Actual-owner TLS preparation

The resource-reference implementation replaces detached client/config mapping
entries with actual `Arc<LinkSys>` / `PreparedLinkSysTls` construction. Etcd's
candidate proves source, whole active local pairing and consumed inherited axes
before fingerprint/RV reuse and zero-live planning. Inherited axes enter the
secret/material digest; a same resourceVersion does not hide global TLS changes.
Absent/disabled local TLS consumes no global axes even for HTTPS endpoints,
leaving endpoint TLS handling to the upstream SDK. Enabled TLS rejects
unsupported verify/hostname/SAN policies during cold construction before dialing.

One immutable connection input retains prepared ordinary/election options and
actual owner across init, probe recovery and dedicated election connections.
Election excludes unary request timeout; ordinary request-only options retain
SDK-native absent dial timeout, and an otherwise empty election uses the existing
bounded dial fallback. The SDK may internally clone/parse PEM during dialing.
Controller uses an explicit trusted disk/YAML bootstrap input through the same
private TLS core, without fabricated Secret/reference authority. See
[ConfCenter bootstrap boundary](../../conf-sync/intentional-design/conf-center-etcd-abstraction.md).
This migration does not add SDK guarantees, live handshake evidence or new
Gateway API conformance claims.
