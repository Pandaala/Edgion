---
name: all-endpoint-status-tls-identity
description: Review AllEndpointStatus Service-port proof, BackendTLSPolicy enforcement, and request-scoped endpoint client isolation.
knowledge_type: regression-guards
---

# AllEndpointStatus backend TLS and endpoint identity

## Conclusion and scope

AllEndpointStatus consumes the canonical published Service section and
`BackendTlsPolicyLookup`. `HTTPBackendRef.backend_tls_policy` is a skipped
runtime-only schema field and is not the routed request's policy source.
An absent policy permits HTTP; an invalid policy produces failed probe results
without dialing; an effective policy requires CA, hostname/SNI, additional SAN,
and client-identity enforcement. This is an on-demand probe contract, not the
periodic active-health-check lifecycle.

## Rationale and evidence

D0032 originally used `backend_tls_policy.is_some()` to choose only the URL
scheme. The normal HTTP path had moved policy capture off that schema field,
so a valid TLS backend could receive plaintext probes.

- `plugins/http/all_endpoint_status/plugin.rs::resolve_endpoints` resolves
  `backendRefs.port` through the immutable discovery generation and uses the
  resulting section name for one policy lookup. Each ready slot supplies its
  own serving port. Missing, zero, negative, overflow, or unproved ports fail
  before dialing rather than falling back to port 80 or another section.
- `plugins/http/all_endpoint_status/transport.rs::ProbeTransport` lazily builds
  one verified TLS configuration per Service/port group in an invocation.
  Backend policy identity wins over the matched Gateway backend identity.
  Explicit defaults passed to `resolve_outbound_tls_with_defaults` prevent
  unrelated LinkSys global trust/client identity from leaking into this path.
- `tls/validation/outbound_client.rs` is the shared verified HTTPS builder
  extracted from OIDC. Both consumers reuse `OutboundSanVerifier`; clients
  pass through the common outbound factory, which rejects competing TLS inputs.
- `EndpointPinnedResolver` binds a client to one socket and rechecks canonical
  address safety at resolution. TLS hostname remains the URL authority, while
  the dial destination is the proven endpoint. Proxy pickup and redirects are
  disabled. Dependency error text is never exposed in endpoint failures.
  The parsed TLS URL host must remain a DNS domain: numeric/IP spellings are
  rejected because reqwest bypasses custom resolution for IP-literal URLs.

Authority: `skills/00-design-rules.md` sections A, B, D, F, U, and X;
`skills/01-architecture/02-gateway/07-backends.md` BackendTLSPolicy contract.

## Alternatives and limits

The user approved per-invocation clients instead of a persistent pool/cache with
invalidation. Each endpoint owns a client because reqwest pools by URL authority:
sharing a client across same-hostname endpoints can probe one endpoint repeatedly.
TLS configuration is shared only within a Service group in that invocation.
This trades cross-request connection reuse for simple certificate-generation
isolation in a bounded, rate-limited diagnostic operation.

Before queued work begins, the captured discovery and policy generations and any
Gateway fallback identity are checked. Generation changes fail queued work;
already-started exchanges finish with their captured configuration. There is no
claim of revoking bytes already sent. Unrelated discovery/policy updates can
conservatively cancel queued probes. Missing Service mappings return HTTP 503;
TLS failures retain HTTP 200 aggregation with null endpoint status/body.

## Verification and re-evaluation

Verified on 2026-09-07 with the supported default build:

- Gateway focused tests: AllEndpointStatus 18 passed, shared TLS handshakes 2
  passed, pinned resolvers 4 passed, OIDC SAN regression tests 3 passed.
- Common LinkSys tests: 91 passed, 4 ignored, including explicit TLS defaults
  and competing client TLS inputs.
- Existing AllEndpointStatus integration suite: 10/10 passed.
- Real shared-topology `AllEndpointStatusTls` dynamic scenario passed: two
  distinct serving ports, TLS/mTLS, hostname/SAN/CA-reference rejection and
  recovery, missing CA, missing client identity, plaintext rejection while
  policy is present, and HTTP only after policy deletion. Cleanup passed.
- Workspace check, Clippy, formatting, documentation consistency, SSA force,
  metrics inventory, Pingora source policy, and gateway tracing guards passed.

The dynamic scenario changes CA references, not Secret contents; it does not
claim real-server coverage of Gateway fallback identity rotation. Client
isolation and generation checks are the implementation evidence for those
lifecycle boundaries. Full workspace tests were not run.

Re-evaluate if clients become persistent, reqwest pooling identity changes,
Service generation publication changes, or policy/identity ownership changes.
Do not use old `DefaultHasher`/removed endpoint-store review conclusions as
evidence about this probe path; their cited mechanisms have changed.

## Resolution references

- D0032 — real correctness/security defect; fixed and verified.
- Task: `add-all-endpoint-status-backend-tls-policy-support`.
