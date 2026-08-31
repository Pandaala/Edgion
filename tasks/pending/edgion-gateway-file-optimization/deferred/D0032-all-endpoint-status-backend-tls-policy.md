# D0032 — Add BackendTLSPolicy-aware AllEndpointStatus Probes

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0063, AllEndpointStatus audit
- Category: correctness-security
- Canonical task: [`tasks/todo/add-all-endpoint-status-backend-tls-policy-support/`](../../../todo/add-all-endpoint-status-backend-tls-policy-support/add-all-endpoint-status-backend-tls-policy-support.md)

## Evidence
AllEndpointStatus performs on-demand fan-out directly to resolved Service endpoints. Its route snapshot does not currently carry an effective BackendTLSPolicy contract for this consumer, so probes ordinarily use plaintext even when normal backend traffic requires TLS. A TLS-only backend can be reported unavailable, and merely changing the URI scheme would omit trust, identity, policy-generation, and fail-close semantics.

## Why deferred
The plugin must consume the canonical absent/effective/invalid policy publication outcome and checked section/port mapping, then apply CA, SNI, SAN, and client identity through the shared backend-independent outbound TLS mechanism. It also needs managed outbound/egress reuse, connection-pool isolation and hot replacement by policy generation, and fail-close behavior without plaintext downgrade. The active-health-check TLS task covers periodic backend health tasks with a different lifecycle and does not own this on-demand plugin.

## Integration-test impact
Existing integration tests remain frozen. Real TLS probe coverage belongs to the technical task after its policy and connection-lifecycle design is approved; B0063's local correctness fixes require no new integration issue.
