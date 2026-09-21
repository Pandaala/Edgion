---
name: ca-bundle-multi-pem
description: mTLS CA loading parses every PEM block in the CA bundle via parse_canonical_ca_bundle, on both downstream paths, at generation-publication time. Reject re-reports claiming only the first block / intermediates are dropped.
knowledge_type: regression-guards
---

# tls-06 — mTLS CA bundle loads every PEM block

**Status:** ✅ fixed · 2026-05-20
**RFC:** RFC 5280 §6.1 (path validation requires the full intermediate chain)

## Conclusion

Both downstream mTLS CA-loading paths parse the whole CA bundle and add **every** certificate to the `X509Store`:

- EdgionTls `clientAuth.caSecretRef`
- Gateway `frontendValidation` CA secrets

Both parse through `edgion_common::tls_validation::parse_canonical_ca_bundle`.
They no longer share one free function, and neither runs at handshake time any
more — each compiles its store when its own generation is published:

| Path | Compiled in | Helper |
|---|---|---|
| EdgionTls | `TlsStore` publication (`compile_client_ca_store`) | `add_ca_bundle_to_store` in `tls/store/compiled_tls.rs` |
| Gateway `frontendValidation` | matcher rebuild (`runtime/matching/tls.rs`) | inlines `parse_canonical_ca_bundle` with its own aggregate byte/count caps |

The load-every-PEM-block invariant below is what both must satisfy; the helper
they reach it through is not itself the rule.

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
- Empty / unparseable bundles stay **fail-closed** on both paths, preserving the
  old `from_pem("")` → error behavior (a misconfigured CA secret cannot silently
  yield an empty trust store). On the EdgionTls path the failure is produced at
  publication and replayed at handshake with identical text; the resource keeps
  its matcher position and fails closed rather than yielding the hostname to
  another candidate.
- Runs at generation-publication time, never on the handshake or per-request
  proxy hot path; no `tracing::*` added.
- Gateway API conformance unaffected: multi-PEM mTLS CA bundles are not exercised
  by the GATEWAY-TLS conformance suite.

## Regression guard

Unit tests in `edgion-gateway/src/tls/store/compiled_tls.rs`:
- `ca_bundle_loads_all_pem_blocks` — 3-block bundle (rcgen self-signed) asserts
  all three certs are added.
- `ca_bundle_rejects_empty_or_garbage` — empty and non-PEM input both error.

Integration test (`edgion-tests/src/client/suites/edgion_tls/mtls/mtls.rs`):
- `test_cert_chain_intermediate_in_ca_store` — connects to `mtls-chain.example.com`
  (EdgionTls `mtls-test-chain`, `caSecretRef: ca-chain` = root+intermediate bundle,
  Mutual, verifyDepth 2) with the **leaf-only** `chain-client.crt` (the
  intermediate is NOT presented by the client). The handshake can only complete
  if the gateway loaded the intermediate from the multi-PEM bundle. Pre-fix this
  failed with "unable to get local issuer certificate"; post-fix it returns 200.
  The leaf-only cert is emitted by `generate_mtls_certs.sh` (`chain-client.crt`,
  distinct from the existing `chain-client-bundle.crt` which bundles the
  intermediate for the older `test_cert_chain_depth` case).

## Related (backend direction, also closed)

The **backend / upstream** direction (`BackendTLSPolicy`, `build_ca_chain` in
`backends/peer_builder.rs`) had the same single-block `X509::from_pem` pattern
(plus a silent parse-error swallow + fail-open). It was a distinct, lower-severity
path — backend servers normally present their own intermediates, so a root-only
trust anchor usually suffices. Fixed separately as **tls-08**; see
[backend-ca-bundle-multi-pem.md](backend-ca-bundle-multi-pem.md).
Do not re-file backend CA bundle reports against tls-06.

## Re-evaluation triggers

- A new mTLS CA source is added — parse it with `parse_canonical_ca_bundle` and
  load every block, never a fresh `X509::from_pem`.
- Either path moves CA-store construction back onto the handshake path.

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a4.md → V09`
- Related (distinct): [terminate-mtls-ca-ordering.md](terminate-mtls-ca-ordering.md)

## Resolution references

- `tls-06` — 2026-05-20.
