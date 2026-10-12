---
name: backend-ca-bundle-multi-pem
description: Backend TLS preparation parses every PEM block and preserves custom-trust fail-closed behavior during generation publication.
knowledge_type: regression-guards
---

# tls-08 — Backend (upstream) TLS CA bundle loads every PEM block + fail-closed

**Status:** ✅ fixed · 2026-05-21
**RFC:** RFC 5280 §6.1 (path validation requires the full intermediate chain)

## Conclusion

`build_ca_chain` in `edgion-gateway/src/backends/prepared_backend_tls.rs` (the trust anchors
used to verify the **backend** server cert under `BackendTLSPolicy`) now:

- parses the whole `ca.crt` bundle with `X509::stack_from_pem` and `extend`s the
  chain with **every** certificate (was `X509::from_pem` → first block only); and
- propagates a parse error / empty stack instead of silently swallowing it
  (`if let Ok(ca)` is gone), returning `EdgionErrorCode::BackendTlsCaInvalid` (502).

`create_tls_peer` now **fails closed** when `use_system_ca == false`
(`wellKnownCACertificates != System`) and no CA can be built: it returns
`BackendTlsCaInvalid` instead of leaving `peer.options.ca` unset and silently
falling back to the system trust store the operator opted out of.

## Why the fix is correct / safe

- Loading more CAs is a **superset** of the old trust set: a single-cert bundle
  still parses identically; multi-block bundles can only make previously
  unbuildable chains validate.
- Fail-closed direction only: a deployment that came up "by accident" via the
  silent system-CA fallback despite a broken `caCertificateRefs` Secret now
  errors clearly. `use_system_ca == true` is completely unchanged.
- Mirrors the frontend tls-06 policy (empty/garbage stays fail-closed); the two
  paths now diverge nowhere semantically.
- ARCH-017 moves parsing from peer construction to winning-policy publication; no `tracing::*` is added.
- Gateway API conformance unaffected: BackendTLSPolicy conformance (including
  `SupportBackendTLSPolicySANValidation`, which **is** declared in
  `conformance_test.go`) exercises a valid single-block CA that parses
  identically and never hits the fail-closed branch; the SAN-validation hook in
  publication is independent of CA loading. Each handshake still computes its own verdict.

## Code shape today

- `prepared_backend_tls.rs` `build_ca_chain` → `Result<Option<Arc<CaType>>, EdgionErrorCode>`,
  `X509::stack_from_pem` + `extend`, empty/unparseable `ca.crt` → `BackendTlsCaInvalid`.
- `PreparedBackendTls` retains an independent CA outcome; `create_tls_peer` reads it under `!use_system_ca`, mapping missing combined trust to `BackendTlsCaInvalid`.
- `EdgionErrorCode::BackendTlsCaInvalid` is recorded when this error reaches the request path.

## Verification boundary

The original tls-08 correction added no focused parser test. ARCH-017 adds a
native preparation unit test for complete multi-PEM bundles, malformed/empty
bundles, missing effective material and system trust ignoring unused invalid CA.
The real BackendTLS suite remains the handshake coverage; parser tests do not
establish certificate-chain handshake acceptance. Fresh execution results are
recorded in the active prepared-generation task, separate from historical evidence.

## Re-evaluation triggers

- A new backend CA source is added — route it through `stack_from_pem` + the same
  fail-closed shape, not a fresh `X509::from_pem`.

## Reference

- Frontend sibling (closed): [ca-bundle-multi-pem.md](ca-bundle-multi-pem.md)
- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a4.md → V09` (frontend pattern); backend extension confirmed by two subagents during tls-06 closure

## Resolution references

- `tls-08` — 2026-05-21.

## BackendTLSPolicy reference/material pairing

Before publishing a backend TLS policy, the cold policy store validates actual
CA ordinals and client-option material through `BackendTlsRefs`. The compressed
CA material vector is matched by canonical namespace/name and Secret/ConfigMap
provenance, never name alone. Missing, surplus, ambiguous or wrong-source
material cannot satisfy a declaration; status may still report genuine partial
CA success. The native maximum of eight references and material cardinality
checks precede scans. Existing Common outbound TLS consumers retain strict
namespace equality and their existing error priority after sharing the material
identity comparator.

This gate establishes reference/material identity, not PEM validity or live TLS
handshake success. PEM-stack semantics and pool-profile identity remain unchanged; native parsing
is now owned by winning-generation preparation. Re-evaluate if the material transport shape, supported source
kinds, local namespace projection, or cold publication boundary changes. Native
CRDs do not declare CA-list uniqueness; duplicate declaration rejection remains
existing Edgion handler behavior and requires separate conformance analysis.
