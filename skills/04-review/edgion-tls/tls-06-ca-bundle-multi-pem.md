---
name: tls-06-ca-bundle-multi-pem
description: mTLS CA loading now parses every PEM block in the CA bundle via X509::stack_from_pem. Reject re-reports claiming only the first block / intermediates are dropped.
---

# tls-06 — mTLS CA bundle loads every PEM block

**Status:** ✅ fixed · 2026-05-20
**RFC:** RFC 5280 §6.1 (path validation requires the full intermediate chain)

## Conclusion

Both mTLS CA-loading paths in `src/core/gateway/tls/runtime/gateway/tls_pingora.rs`
now parse the whole CA bundle with `X509::stack_from_pem` and add **every**
certificate to the `X509Store`:

- `configure_mtls` (EdgionTls `clientAuth.caSecretRef`)
- `configure_gateway_frontend_mtls` (Gateway `frontendValidation` CA secrets)

Both delegate to the shared free fn `add_ca_bundle_to_store`.

Re-reports of the form "CA bundle loads only the first PEM block",
"intermediates in `ca.crt` are silently discarded", or "use
`X509::stack_from_pem` instead of `X509::from_pem`" are **not accepted** — they
are no longer true.

## What was wrong (before the fix)

`X509::from_pem(ca_pem.as_bytes())` parses exactly one PEM block. A standard
Kubernetes CA secret whose `ca.crt` holds a `root + intermediate` bundle had
every block after the first silently dropped, so any client cert signed by the
intermediate failed verification with no diagnostic pointing at the partial
CA load.

## Why the fix is correct / safe

- Loading more CAs into the trust store is a **superset** of the old trust set:
  it can only make previously-unbuildable chains validate, never break a
  single-cert bundle (which still parses identically).
- Empty / unparseable bundles stay **fail-closed**: `add_ca_bundle_to_store`
  returns `InvalidCert` when the parsed stack is empty, preserving the old
  `from_pem("")` → error behavior (a misconfigured CA secret cannot silently
  yield an empty trust store).
- Runs only at handshake / cert-callback configuration time, not the per-request
  proxy hot path; no `tracing::*` added.
- Gateway API conformance unaffected: multi-PEM mTLS CA bundles are not exercised
  by the GATEWAY-TLS conformance suite.

## Regression guard

Unit tests in `tls_pingora.rs`:
- `ca_bundle_loads_all_pem_blocks` — 3-block bundle (rcgen self-signed) asserts
  all three certs are added.
- `ca_bundle_rejects_empty_or_garbage` — empty and non-PEM input both error.

Integration test (`examples/code/client/suites/edgion_tls/mtls/mtls.rs`):
- `test_cert_chain_intermediate_in_ca_store` — connects to `mtls-chain.example.com`
  (EdgionTls `mtls-test-chain`, `caSecretRef: ca-chain` = root+intermediate bundle,
  Mutual, verifyDepth 2) with the **leaf-only** `chain-client.crt` (the
  intermediate is NOT presented by the client). The handshake can only complete
  if the gateway loaded the intermediate from the multi-PEM bundle. Pre-fix this
  failed with "unable to get local issuer certificate"; post-fix it returns 200.
  The leaf-only cert is emitted by `generate_mtls_certs.sh` (`chain-client.crt`,
  distinct from the existing `chain-client-bundle.crt` which bundles the
  intermediate for the older `test_cert_chain_depth` case).

## Out of scope (tracked separately)

The **backend / upstream** direction (`BackendTLSPolicy`, `build_ca_chain` in
`backends/peer_builder.rs`) has the same single-block `X509::from_pem` pattern
(plus a silent parse-error swallow). It is a distinct, lower-severity path —
backend servers normally present their own intermediates, so a root-only trust
anchor usually suffices. Tracked as **tls-08**
(`tasks/todo/rfc-review/rfc-violation-tls-08-backend-ca-bundle-only-first-pem-block.md`);
do not re-file it against tls-06.

## Re-evaluation triggers

- A new mTLS CA source is added — route it through `add_ca_bundle_to_store`, not
  a fresh `X509::from_pem`.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a4.md → V09`
- Closure: [CLOSED-FINDINGS.md → tls-06](../CLOSED-FINDINGS.md)
- Related (distinct): [tls-F001-terminate-mtls-ca-ordering.md](tls-F001-terminate-mtls-ca-ordering.md)
