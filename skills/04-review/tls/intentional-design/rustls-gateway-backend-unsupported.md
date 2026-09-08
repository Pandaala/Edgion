---
name: rustls-gateway-backend-unsupported
description: Use when reviewing rustls-only Gateway build, plugin portability, downstream TLS, upstream policy, Docker, CI, or test-parity findings; the Pingora rustls backend is future product scope.
status: accepted-tradeoff
finding_id: rustls-gateway-backend-support
closed: 2026-08-30
knowledge_type: intentional-design
---

# Rustls Gateway Backend — FUTURE PRODUCT SCOPE

## Conclusion

BoringSSL is Edgion's only supported Pingora Gateway TLS backend. The retained
`rustls` Cargo feature is a dormant future-development seam, not a clean-build,
runtime, packaging, CI, integration-test, or parity promise.

Fix suggestions of the form "repair this isolated rustls-only compile failure,
warning, plugin incompatibility, TLS hook, Docker command, or test gap" are **not
accepted** outside the explicitly authorized umbrella task.

Edgion will not implement Gateway rustls support until Pingora provides complete
rustls support for the required downstream and upstream TLS surface. The pending
task remains blocked on this prerequisite; partial APIs or a compiling feature
do not satisfy it, and Edgion must not fill the gaps with isolated parity patches.

## Core Rationale

**1. Frontend TLS support is incomplete by product decision**

The rustls path does not implement Gateway downstream certificate automation,
dynamic SNI selection, custom termination callbacks, EdgionTls behavior, or
TLSRoute termination. Making one leaf module compile would not create a usable
or supportable Gateway TLS backend.

**2. Partial parity creates a misleading security contract**

Rustls-only code also lacks complete upstream BackendTLSPolicy custom trust,
client-identity, and additional-SAN enforcement. Advertising a clean build or
adding isolated CI coverage would imply a supported surface whose TLS policy
semantics are incomplete.

**3. HeaderCertAuth does not justify a narrow backend project**

Header mode could technically parse certificates without a TLS-backend API,
but porting its subject, SAN, CN, and fingerprint extraction only to restore a
rustls-only build has no current product value. Its default BoringSSL behavior
remains supported and unchanged.

## Fix Suggestions Not Accepted

- Replacing HeaderCertAuth's BoringSSL X509 calls solely to make rustls compile.
- Adding a zero-backend diagnostic or rustls-only CI job as an isolated patch.
- Fixing rustls Docker, documentation, or integration-harness parity before the
  runtime product boundary is designed.
- Treating upstream CA, mTLS, or SAN gaps as narrow security bugs on an
  unsupported Gateway backend.

## Re-evaluation Triggers

Re-open this decision only when all of the following hold:

- Pingora provides complete rustls support for the required downstream and
  upstream TLS surface, verified against the revision used by Edgion;
- the user explicitly authorizes `tasks/pending/rustls-gateway-tls-support.md`;
- the complete downstream and upstream TLS capability boundary, failure policy,
  packaging, CI, and integration strategy has been designed and approved.

## Reference Cases

- `rustls-build-and-feature-contract` closed as an accepted product-boundary
  decision on 2026-08-30.
- `AGENTS.md`
- `skills/01-architecture/02-gateway/04-tls/03-ssl-backends.md`
- `tasks/pending/rustls-gateway-tls-support.md`

## Resolution references

- `rustls-gateway-backend-support` — 2026-08-30 (accepted tradeoff).
