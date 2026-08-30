---
name: rustls-upstream-ca-mtls-unsupported
description: Use when reviewing rustls upstream BackendTLSPolicy CA or client-identity parity findings; these capabilities are outside the current rustls support boundary.
status: accepted-tradeoff
finding_id: rustls-upstream-ca-mtls
closed: 2026-08-30
---

# Rustls Upstream Custom CA and mTLS — UNSUPPORTED

## Conclusion

Per-peer custom CA trust and client-certificate authentication for upstream
`BackendTLSPolicy` connections are not currently supported by Edgion's rustls
build. BoringSSL remains the supported Gateway TLS backend for these capabilities.

Fix suggestions of the form "add rustls CA and mTLS parity as a narrow security
bugfix" are **not accepted** while rustls upstream policy support remains outside
the product boundary.

## Core Rationale

**1. The product boundary explicitly excludes this capability**

`create_tls_peer` creates a rustls TLS peer with the policy hostname, but the
custom CA and client-identity construction paths are BoringSSL-gated. This is a
known capability boundary, not a promise that rustls enforces the complete
`BackendTLSPolicy` trust and identity contract.

**2. Enabling parity is larger than a local parser change**

A supported implementation must validate operator-controlled CA, certificate,
and private-key material before entering fork constructors; preserve policy and
Gateway fallback precedence; and isolate pooled connections across effective
trust and identity changes. It also requires a clean rustls-only build and a
backend-selectable integration harness.

**3. BoringSSL behavior remains authoritative**

The accepted boundary does not weaken the implemented BoringSSL path. BoringSSL
continues to enforce custom CA and upstream client-certificate behavior, and a
Gateway-level client identity is only a fallback after a `BackendTLSPolicy` has
selected TLS. Complete connection-pool isolation across CA and client-identity
changes remains a separate BoringSSL-relevant security task and is not accepted
or closed by this rustls decision.

## Fix Suggestions Not Accepted

- Reusing the BoringSSL-only PEM parsers behind loose feature gates — rustls has
  different parsed types and panic-prone constructor boundaries.
- Installing CA or client identity without a complete connection-pool key — a
  pooled connection must not survive an effective TLS-policy change.
- Expanding this finding into rustls build or integration-harness work — those
  are separate product tasks and do not make this capability supported today.

## Re-evaluation Triggers

Re-open this decision only if:

- Edgion declares upstream `BackendTLSPolicy` custom CA or mTLS support for the
  rustls build.
- A release, deployment profile, or test matrix begins selecting rustls for a
  Gateway that may receive either of these policy settings.
- The rustls build and backend-selectable integration prerequisites are
  completed and product scope explicitly authorizes parity work.

## Reference Cases

- Product decision recorded on 2026-08-30 while closing
  `rustls-upstream-ca-mtls` as an accepted unsupported capability.
- `edgion-gateway/src/backends/peer_builder.rs`
- `skills/01-architecture/02-gateway/04-tls/03-ssl-backends.md`
- `tasks/todo/backend-tls-policy-connection-pool-isolation.md`
