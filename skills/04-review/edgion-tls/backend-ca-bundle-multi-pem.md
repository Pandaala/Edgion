---
name: backend-ca-bundle-multi-pem
description: Backend (upstream) TLS CA loading now parses every PEM block via X509::stack_from_pem and fails closed when use_system_ca=false yields no CA. Reject re-reports claiming only the first block / intermediates are dropped, or that a missing CA silently falls back to the system store.
---

# tls-08 — Backend (upstream) TLS CA bundle loads every PEM block + fail-closed

**Status:** ✅ fixed · 2026-05-21
**RFC:** RFC 5280 §6.1 (path validation requires the full intermediate chain)

## Conclusion

`build_ca_chain` in `src/core/gateway/backends/peer_builder.rs` (the trust anchors
used to verify the **backend** server cert under `BackendTLSPolicy`) now:

- parses the whole `ca.crt` bundle with `X509::stack_from_pem` and `extend`s the
  chain with **every** certificate (was `X509::from_pem` → first block only); and
- propagates a parse error / empty stack instead of silently swallowing it
  (`if let Ok(ca)` is gone), returning `EdgionStatus::BackendTlsCaInvalid` (502).

`create_tls_peer` now **fails closed** when `use_system_ca == false`
(`wellKnownCACertificates != System`) and no CA can be built: it returns
`BackendTlsCaInvalid` instead of leaving `peer.options.ca` unset and silently
falling back to the system trust store the operator opted out of.

Re-reports of the form "backend CA bundle loads only the first PEM block",
"intermediates in `ca.crt` are silently discarded", "use `X509::stack_from_pem`",
or "missing/garbage backend CA silently uses the system store" are **not
accepted** — they are no longer true.

## Why the fix is correct / safe

- Loading more CAs is a **superset** of the old trust set: a single-cert bundle
  still parses identically; multi-block bundles can only make previously
  unbuildable chains validate.
- Fail-closed direction only: a deployment that came up "by accident" via the
  silent system-CA fallback despite a broken `caCertificateRefs` Secret now
  errors clearly. `use_system_ca == true` is completely unchanged.
- Mirrors the frontend tls-06 policy (empty/garbage stays fail-closed); the two
  paths now diverge nowhere semantically.
- Runs at peer-build time (not added to any hot path); no `tracing::*` added.
- Gateway API conformance unaffected: BackendTLSPolicy conformance (including
  `SupportBackendTLSPolicySANValidation`, which **is** declared in
  `conformance_test.go`) exercises a valid single-block CA that parses
  identically and never hits the fail-closed branch; the SAN-validation hook in
  `create_tls_peer` is independent of CA loading and untouched.

## Code shape today

- `peer_builder.rs` `build_ca_chain` → `Result<Option<Arc<CaType>>, EdgionStatus>`,
  `X509::stack_from_pem` + `extend`, empty/unparseable `ca.crt` → `BackendTlsCaInvalid`.
- `peer_builder.rs` `create_tls_peer` → `match build_ca_chain(policy)? { None => Err(BackendTlsCaInvalid), .. }` under `!use_system_ca`.
- `EdgionStatus::BackendTlsCaInvalid = 502_03`.

## Out of scope (intentional)

No integration/unit test added: `build_ca_chain` takes a fully-resolved
`BackendTLSPolicy` and the backend leaf-only scenario needs a test server
configured to withhold its intermediate (high setup, low value — real servers
present their full chain). The valid-CA happy path stays covered by the existing
BackendTLSPolicy conformance suite.

## Re-evaluation triggers

- A new backend CA source is added — route it through `stack_from_pem` + the same
  fail-closed shape, not a fresh `X509::from_pem`.

## Reference

- Frontend sibling (closed): [ca-bundle-multi-pem.md](ca-bundle-multi-pem.md)
- Verifier: `tasks/todo/rfc-review/_raw/verification-a4.md → V09` (frontend pattern); backend extension confirmed by two subagents during tls-06 closure
