---
name: rustls-upstream-san-unsupported
description: Use when reviewing rustls upstream BackendTLSPolicy subjectAltNames parity findings; additional Hostname and URI constraints are outside the current rustls support boundary.
status: accepted-tradeoff
finding_id: rustls-upstream-san-enforcement
closed: 2026-08-30
knowledge_type: intentional-design
---

# Rustls Upstream Additional SAN Enforcement — UNSUPPORTED

> Scope: the entire Pingora Gateway rustls backend is unsupported. Read
> [the current product boundary](rustls-gateway-backend-unsupported.md) before
> applying the historical backend-specific details below.

## Conclusion

Additional `BackendTLSPolicy.spec.validation.subjectAltNames` Hostname and URI
constraints are not currently supported by Edgion's rustls build. BoringSSL
remains the supported Gateway TLS backend for this capability; rustls still
performs the standard `validation.hostname` check through WebPKI.

Fix suggestions of the form "add rustls additional-SAN parity as a narrow
security bugfix" are **not accepted** while complete rustls upstream policy
support remains outside the product boundary.

## Core Rationale

**1. The current backend-specific behavior is explicit**

`create_tls_peer` installs the additional-SAN handshake hook only in the
BoringSSL build. Its pool discriminator now covers the complete TLS profile,
including SANs; see the [pool isolation regression guard](../regression-guards/backend-tls-pool-isolation.md).
The pinned Pingora
fork exposes that handshake-result hook only for OpenSSL-derived backends, so a
rustls peer has no equivalent result for `connected_to_upstream` to consume.

**2. Standard hostname verification remains active**

The accepted boundary does not disable rustls certificate-chain, validity, or
standard server-name verification. It is limited to the separately configured
Hostname and URI entries in `subjectAltNames`.

**3. Correct parity requires a composable fork integration**

A supported implementation must add an unambiguous rustls verification seam,
reuse the existing BackendTLSPolicy SAN matcher, fail closed when validation is
unavailable, and compose its pool discriminator with custom-CA and client-
identity policy. It also requires a clean rustls-only build and backend-
selectable integration coverage for allowed, denied, missing, and malformed
certificates.

## Fix Suggestions Not Accepted

- Reusing the BoringSSL handshake hook behind a looser feature gate — rustls
  does not expose an OpenSSL `SslRef` or that hook contract.
- Installing a rustls verifier without composing the complete effective TLS
  policy into connection reuse — a pooled connection must not cross SAN,
  custom-CA, or client-identity policy changes.
- Treating the existing rustls `validation.hostname` verification as equivalent
  to `subjectAltNames` — they are distinct constraints with different values.

## Re-evaluation Triggers

Re-open this decision only if:

- Edgion declares additional upstream `BackendTLSPolicy` SAN enforcement for
  the rustls build.
- A release, deployment profile, or conformance claim selects rustls while
  allowing `subjectAltNames` on upstream policies.
- The rustls build, effective-policy pool key, and backend-selectable
  integration prerequisites are completed and product scope authorizes parity.

## Reference Cases

- Product decision recorded on 2026-08-30 while closing
  `rustls-upstream-san-enforcement` as an accepted unsupported capability.
- `edgion-gateway/src/backends/peer_builder.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_connected_to_upstream.rs`
- `skills/04-review/tls/intentional-design/rustls-upstream-ca-mtls-unsupported.md`

## Resolution references

- `rustls-upstream-san-enforcement` — 2026-08-30 (accepted tradeoff).
