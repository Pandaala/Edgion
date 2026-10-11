---
id: ARCH-059
status: candidate
kind: architecture-debt
priority: P3
primary: edgion-resources/src/resources/health_check.rs
partition: PART-201
analyst: /root
baseline: BASELINE-AUDIT-20261010-D.json
---

# Reuse the existing Kubernetes name predicates for health-check references

`ActiveHealthCheckConfig::tls_validation_error` checks CA and client certificate
reference names and explicit namespaces using its private
`valid_reference_name` and `valid_reference_namespace` functions (204–216).
These implement the same lowercase ASCII DNS-subdomain/label predicates and
253/63-byte limits already owned by
`resources/common/kubernetes_name.rs::is_dns_subdomain` and `is_dns_label`.
The health hostname uses the subdomain predicate too, followed by a separate
`url::Host::Domain` check that rejects numeric IP spellings.

The existing shared predicates serve actual reference consumers:
`refs/identity.rs::ObjectIdentity::checked`, OTLP Secret validation,
ProxyProtocolTrustResourceRef validation, and EdgionBackendTrafficPolicy target
names. A reference-name rule correction or boundary-test change currently has
to account for the health copy separately. Controller
`handlers/edgion_backend_traffic_policy.rs::health_tls_shape_error` and
`resolve_health_tls`, and Gateway `EncryptedProbeTransport::build`, consume
the health validator, so keeping the copy creates another configuration-policy
owner. No differing acceptance or current security incident is alleged.

The legitimate health-specific restrictions stay in the health owner: verified
TLS, supported CA kinds, same-namespace references, SAN checks, exclusive
system/explicit trust, and URL numeric-host rejection. The recorded independent
health transport design expressly requires these restrictions and preserves a
winning policy when TLS material fails. That is separate from lexical name
validation and must not be merged into ordinary backend attachment policy.

Replace the local lexical helpers with calls to the existing crate-private
predicates using exactly 63 and 253. Retain health-specific error strings,
validation order, DNS-only URL classification, and same-namespace checks. This
requires no new public API, abstraction, dependencies, allocation, transport
field, or change to schema. Leaving tiny wrapper functions that merely delegate
is an acceptable alternative, although deleting the copies makes ownership
clearer. Do not expand this finding into all resource-reference validation.

The benefit is one existing owner for the same lexical mechanism. This is the
graded reuse-first recommendation, not a claim that DNS name validation is a
new mandatory entry in the outbound DNS-resolver unification registry. The
registry's extraction threshold is irrelevant to creating a new abstraction:
the abstraction already exists and is consumed in production.

Future verification should compare old/new acceptance for empty input,
uppercase, non-ASCII UTF-8, underscores, leading/trailing hyphens, empty labels,
63/64-byte labels and 253/254-byte names, and preserve numeric-host rejection,
wildcard rejection, same-namespace trust, unresolved-material failure, rotation,
redaction and policy eligibility. Existing Resources health tests, Controller
encrypted-health attachment/material tests and Gateway endpoint-pin/mTLS tests
were inspected as source. No tests or runtime checks were executed.

Original knowledge consulted includes graded rules E/J, the unification registry,
`backends/intentional-design/active-health-check-transport-is-independent.md`,
`backends/regression-guards/active-health-port-validation.md`, and the owning
reference-identity exception in
`plugin-runtime/regression-guards/request-template-policy-unification.md`.
Searches of review knowledge, pending tasks and earlier ARCH artifacts found no
existing finding for these health lexical helpers. Template Secret grammar
intentionally retains its own lexical acceptance and is outside this scope.

Independent decision pending. Supporting reads confer no additional primary
coverage; exact primary/support hashes are recorded in REPORT-PART-201.json.
