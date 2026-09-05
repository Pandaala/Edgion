# Add BackendTLSPolicy-aware AllEndpointStatus Probes

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | feature |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Make AllEndpointStatus on-demand endpoint probes honor the effective BackendTLSPolicy with the same validated backend identity and fail-close guarantees as ordinary traffic. Do not implement this by changing only the URL scheme or by creating a plugin-local TLS stack.

## Requirements

- Consume the canonical absent, effective, and invalid BackendTLSPolicy publication outcome (`BackendTlsPolicyLookup`) already published by the Gateway store; do not define a plugin-local lookup or collapse invalid/unresolved into absent.
- Consume the exact section and checked Service-to-serving-port outcome owned by the canonical backend-port task before choosing a policy or dialing an EndpointSlice endpoint.
- Apply the effective CA trust, SNI, SAN constraints, and client certificate identity through the canonical backend-independent outbound TLS mechanism used by the default supported Gateway build.
- Reuse the managed outbound client, canonical outbound TLS resolution, egress resolver and endpoint-IP pinning, proxy-env disablement, and redirect denial; do not add a second client/TLS mechanism.
- If the shared outbound path cannot enforce a required SAN constraint, extend that canonical mechanism for every applicable consumer rather than adding a plugin-local verifier or client.
- Isolate connection pools by backend identity and policy/certificate generation so a pooled connection cannot cross trust or client-identity generations.
- Replace or invalidate affected probe clients atomically on BackendTLSPolicy, CA Secret, client identity, Service port, or endpoint generation changes.
- Keep per-request fan-out bounded and preserve the existing wall timeout, body cap, result schema, safe-address checks, and canonical access-log ownership.
- Never include certificate, Secret, target, or policy detail in client-facing errors or high-cardinality logs/metrics.

## Scope

### In scope

- Gateway AllEndpointStatus client selection and generation-safe pool lifecycle.
- Integration of the canonical BackendTLSPolicy tri-state/publication and backend-port outcomes into this consumer.
- Focused default-build unit/integration coverage after the design is approved.

### Out of scope

- Periodic active health-check task scheduling or protocol semantics.
- The unsupported Pingora Gateway `rustls` feature; backend-independent rustls libraries used by the managed outbound client remain in scope.
- A plugin-local CA/SNI parser or a plaintext fallback for invalid TLS policy.
- A duplicate BackendTLSPolicy publication/validation state machine or Service-port resolver.
- Unrelated AllEndpointStatus response-schema changes.

## Acceptance criteria

- A backend without BackendTLSPolicy preserves the documented plaintext behavior.
- A valid policy probes with the expected CA, SNI, SAN, and optional mTLS identity.
- Invalid, denied, unresolved, or generation-mismatched policy state fails closed and never retries over plaintext.
- Exact section/port matching uses the canonical checked Service-to-serving-port mapping.
- Policy and certificate hot updates cannot reuse a connection from the previous trust or identity generation.
- Egress restrictions, redirect denial, proxy-env disablement, wall timeout, endpoint cap, and response body cap remain enforced.
- Client-visible and plugin-log failures reveal no certificate, Secret, policy, or target detail.
- Targeted tests, workspace check, Clippy, repository guards, and approved real TLS integration coverage pass.

## Related work

- [`Precompile backend port resolution and fail closed on missing mappings`](../backend-port-resolution-precompiled-fail-closed.md)
- Canonical BackendTLSPolicy tri-state: `skills/01-architecture/02-gateway/07-backends.md` (BackendTLSPolicy section)
- Campaign finding D0005: active health-check TLS contract (adjacent lifecycle, not this task's owner)

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Create a dedicated on-demand probe TLS task instead of extending B0063 or D0005. | AllEndpointStatus has request-scoped bounded fan-out and policy-generation pool needs distinct from periodic health-check task lifecycle. |
| 2026-09-01 | Consume the canonical BackendTLSPolicy tri-state and port-resolution outcomes. | Publication validity and exact Service-section identity are shared contracts; reimplementing them in this plugin would create unsafe divergent plaintext-downgrade behavior. |
