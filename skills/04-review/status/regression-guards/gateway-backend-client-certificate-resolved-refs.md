---
name: gateway-backend-client-certificate-resolved-refs
description: Use when reviewing Gateway-level ResolvedRefs for spec.tls.backend.clientCertificateRef, or upstream identity PEM validation.
knowledge_type: regression-guards
---

# Gateway backend client certificate drives Gateway ResolvedRefs

## Conclusion and scope

`Gateway.spec.tls.backend.clientCertificateRef` contributes to the single
Gateway-level `ResolvedRefs` condition:

| Resolution outcome | Status / Reason |
|---|---|
| Cross-namespace reference no ReferenceGrant allows | `False` / `RefNotPermitted` |
| Invalid reference identity (group, kind, name, namespace or binding), missing Secret, no data, missing or empty `tls.crt` / `tls.key`, unparseable certificate or key PEM | `False` / `InvalidClientCertificateRef` |
| Both PEMs parse but the key does not match the certificate | `False` / `InvalidCertificate` (Edgion extension) |
| Usable key pair | contributes nothing; `True` if no other gateway-level ref fails |

The Controller owns this in
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`:
`resolve_gateway_tls` records a `ClientCertificateResolutionError` (from
`edgion-resources/src/resources/gateway.rs`), `backend_client_certificate_ref_error`
maps it and the resolved Secret to a reason and message, and `update_status` merges
the result. Both Controller status and Gateway identity preparation call
`edgion_common::tls_validation::parse_gateway_client_identity`. It validates the
ordered certificate chain, selects the first recognized private key, and checks
the leaf public key. OpenSSL bag attributes on the certificate and a leading
EC PARAMETERS block on the key are supported. The Gateway
loads the parsed DER into BoringSSL. Listener serving keys retain their strict
single-key PEM contract.

Inside the merged condition, a backend client-certificate failure supplies the
reason even if an Edgion extension also fails. All messages are retained. Messages are
built from fixed text, the reference identity and, for a mismatch, the validator's
error text, so they are deterministic for a given input.

## Rationale and evidence

The vendored Gateway API v1.6.2 `clientCertificateRef` field
(`config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml`) requires `RefNotPermitted`
for a reference no ReferenceGrant allows and `InvalidClientCertificateRef` for an
unresolvable or misconfigured one. The upstream reason definition
(`GatewayReasonInvalidClientCertificateRef` in `apis/v1/gateway_types.go`) widens
"misconfigured" to a nonexistent or unsupported resource or kind, or malformed data.
Further content validation needs an implementation-specific reason; the key/certificate
match is that further check. The upstream conformance test
`GatewayInvalidTLSBackendConfiguration` (v1.6.2) expects exactly these Gateway-level
reasons for a missing Secret, wrong group, wrong kind, empty data and an ungranted
cross-namespace Secret, with `Accepted=True`. `GatewayBackendClientCertificate` remains
declared unsupported, so the test is not run.

Before this guard, `resolve_gateway_tls` collapsed every non-`Ok` `fetch_bound_secret`
outcome for this field to `None` and `update_status` never examined it, so the Gateway
reported `ResolvedRefs=True` for a denied, missing or malformed reference. Data-plane
effect, unchanged by the fix: with no resolved Secret the Gateway removes its
identity slot and connects without a client certificate, so only a backend that
requires client authentication rejects the handshake (see
[backend client identity](../../tls/regression-guards/backend-client-identity-tls-selection.md));
resolved material that BoringSSL cannot parse fails peer construction. The status
condition is therefore the operator's main signal that the identity is absent.

The resolution outcome lives on `GatewayBackendTLS.resolved_client_certificate_error`,
a `#[serde(skip)]` `#[schemars(skip)]` field: `parse` clears it, `resolve_gateway_tls`
sets it, and `update_status` reads it on the same in-memory object
(`resource_processor/processor.rs`). It never reaches the CRD schema or conf_sync.
Controller-only, non-serialized per-reconcile carriers already exist in
`EdgionBackend.reconcile_outcome` and `RequestAccessPolicy.resolution_warnings`
(consumed by status) and `EdgionAcme.notify_after_publish` (consumed after cache
publication). A transported field would add an unused conf_sync
surface, and re-reading the store in `update_status` could diverge from what parse
resolved.

## Alternatives and limits

- A cross-namespace reference with an invalid group or kind reports
  `InvalidClientCertificateRef` without consulting ReferenceGrants, because identity
  validation precedes authorization. Upstream reserves that reason for allowed
  references; listener `certificateRefs` behave the same way, conformance does not
  cover the combination, and nothing about the target is revealed.
- If a future refactor serialized the Gateway between `parse` and `update_status`, the
  carrier would be lost and a denial would degrade to `InvalidClientCertificateRef`
  ("Secret not resolved"): still `False`, but with the wrong reason.
- A distinct enum is used instead of `ServingCertificateResolutionError`: the variants
  match, but `InvalidReference` carries the `ReferenceError` so the message states the
  concrete cause, and a serving name on a client field would mislead.

## Verification and re-evaluation

- 2026-11-12, `edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/gateway_test.rs`:
  `backend_client_certificate_cross_namespace_grant_drives_gateway_resolved_refs`
  (denied, granted, revoked) and
  `backend_client_certificate_unresolvable_misconfigured_or_unusable_sets_resolved_refs_false`
  (missing Secret, missing key, literal `core` group, ConfigMap kind, unparseable PEM,
  mismatched pair, valid pair) failed with `ResolvedRefs=True` before the fix.
  `backend_client_certificate_reason_outranks_proxy_reference_error_in_one_condition`
  fails when the precedence is reordered, and
  `tls_keypair_secret_check_classifies_and_orders_problems` pins the listener check's
  classification, messages and order. `cargo test -p edgion-controller --lib
  handlers::gateway` passed 112 tests.
- 2026-11-12 FS-mode live run on the shared topology, on `d4fb7f4ad` plus this
  uncommitted fix, before the reason refinement for unparseable PEM: in a
  frontend-CA / backend-client Grant matrix (none, all, deleted, recreated, CA-only,
  client-only, wrong source namespace, all), Gateway-level status was `RefNotPermitted`
  exactly where the client certificate was not granted and `True` otherwise, with
  unchanged traffic. Deleting and restoring the granted Secret moved status to
  `InvalidClientCertificateRef` and back. The same matrix before the fix reported
  `True` in every phase. `run_integration.sh -r Gateway` passed (108 passed, 0 failed,
  4 skipped). Evidence is local and not retained.
- Not covered: Kubernetes-mode status persistence of this condition. Re-evaluate when
  declaring `GatewayBackendClientCertificate`, changing the merged reason precedence,
  adding another gateway-level reference, or changing upstream identity parsing.

- Current regression coverage: the Controller grant lifecycle and Gateway
  identity preparation tests use the same OpenSSL SEC1 key and certificate
  fixtures. `backend_client_certificate_reason_outranks_extension_errors_in_one_condition`
  checks both the native reason and the retained extension messages.

## Resolution references

- 2026-11-12 Grant-matrix finding during the resource reference contract closure
  (see [contract verification evidence](../../../01-architecture/00-common/06-resource-reference-contract.md#verification-evidence)):
  a denied backend client certificate failed in traffic while the Gateway reported
  `ResolvedRefs=True`.
