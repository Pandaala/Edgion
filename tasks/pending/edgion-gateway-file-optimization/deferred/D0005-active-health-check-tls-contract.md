# D0005 — Define the Active Health-check TLS Contract

## Meta

- Status: needs-discussion
- Severity: medium
- Discovered in: B0040, `edgion-gateway/src/backends/health/check/probes.rs`
- Category: architecture

## Evidence

HTTP health probes construct plaintext `http://` requests, and gRPC health probes use a plaintext transport. Neither path consults the effective BackendTLSPolicy. TCP probes only establish connectivity and cannot validate a TLS handshake or backend identity. A TLS-only backend can therefore be reported unhealthy by HTTP/gRPC probing or only superficially reachable by TCP probing.

## Why deferred

Encrypted active probing needs an explicit policy contract for CA trust, SNI, client identity, connection-pool isolation, certificate hot reload, and task respawn. Reusing only one request-peer field or switching a URI scheme would produce incomplete and potentially unsafe TLS semantics. This task excludes unsupported Gateway rustls parity.

## Impact and open questions

- Define whether active probes inherit BackendTLSPolicy or use explicit health-check TLS configuration.
- Define CA/SNI/client-certificate precedence and validation failures.
- Ensure pooled clients cannot cross policy or identity generations.
- Define how policy and certificate reloads replace probe tasks.
- Preserve existing plaintext behavior for backends without TLS policy.

## Candidate next step

Specify the resource and runtime contract first, then add BoringSSL-supported HTTP and gRPC probe tests for CA trust, SNI, mTLS identity, invalid-certificate fail-close behavior, and hot reload. Do not add isolated rustls parity work.
