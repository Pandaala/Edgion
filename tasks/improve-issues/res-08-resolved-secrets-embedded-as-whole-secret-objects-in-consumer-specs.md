# res-08: Resolved secret material is embedded as whole `k8s Secret` objects inside every consuming resource instead of SDS-style named secrets

- **Category**: best-practice
- **Severity**: P3
- **Scope**: large-direction
- **Location**:
  - `edgion-resources/src/resources/common/secret_slot.rs:11-21` (`SecretSlot.secret: Option<Secret>`)
  - `edgion-resources/src/resources/common/tls.rs:65-74`, `common/outbound_tls.rs:99-109`, `health_check.rs:83-98`, `backend_tls_policy.rs:76-85`, `gateway.rs:282,431`, `edgion_tls.rs:127,218`, `plugin_configs/{ldap_auth.rs:43-52,openid_connect.rs:223-231,header_cert_auth.rs:127}` (≈17 `Option<Secret>`/`Vec<Secret>` fields)
  - `edgion-resources/src/redact.rs` (per-field admin redaction adapter), `edgion-resources/src/credentials/materials.rs:13-28` (documents the unprotected copies)

## Summary

The Controller resolves a referenced Secret and writes the complete `k8s_openapi::Secret` (all data keys plus
retained metadata) into a schema-hidden field of each consuming resource; conf_sync then ships the consumer
with the Secret inside. One Secret referenced by N resources is shipped N times, every Secret rotation
reprocesses and re-sends every consumer (including large EdgionPlugins with WAF bundles) and rebuilds their
runtimes, and protection of each copy depends on two attributes (`#[schemars(skip)]` + `redact_opt`) being
remembered on every new field — the 🔴 rule A in `skills/00-design-rules.md` exists because this is easy to
get wrong.

## Current behavior (evidence)

- Field types are the raw Kubernetes object, e.g. `ActiveHealthCheckConfig.resolved_ca_certificates:
  Option<Vec<Secret>>`, `resolved_client_certificate: Option<Secret>`; `SecretSlot.secret: Option<Secret>`.
  Consumers then pick the key they need at runtime (e.g. `EdgionTls::cert_pem/key_pem/ca_cert_pem`,
  `edgion_tls.rs:354-418`).
- Nothing narrows the object to the keys the consumer uses: an Opaque Secret with unrelated keys referenced as
  a CA bundle is copied whole into the data-plane payload. Metadata stripping depends on the configurable
  controller metadata filter (`remove_managed_fields`, last-applied annotation filter).
- `credentials/materials.rs:13-28` documents that the embedded `Secret` copies are serialized through
  serde_json/gRPC and never wiped.
- Rotation path: Secret change → `SecretRefManager` requeues every referencing resource → each is
  re-resolved, re-serialized and re-published; gateways rebuild those resources' compiled state.

## Industry comparison

- Envoy SDS: secrets are separate named xDS resources (`Secret{tls_certificate|validation_context|
  generic_secret}`) carrying only the typed fields; listeners/clusters reference them by name, so a cert
  rotation pushes one small resource and does not touch listener/cluster config.
- Istio/Envoy Gateway follow the same model (`credentialName` → SDS).

## Impact

Payload amplification and rebuild churn proportional to (secret references × consumer size) on every
rotation; broader-than-needed key exposure on the data plane; a fragile, per-field redaction discipline.
It also compounds `arch-01` (every gateway receives every embedded copy).

## Suggested direction

Introduce a single resolved-secret transport kind (Controller-produced, keyed by `namespace/name` + selected
keys, typed payload: `TlsKeyPair`, `CaBundle`, `Opaque{key}`), sync it once, and replace the embedded
`Option<Secret>` fields with a typed handle (`ResolvedSecretRef { key, generation }`). Gateway consumers look
the material up at compile time and subscribe to its generation, as Envoy does with SDS. Redaction then lives on
one type instead of ~17 fields. Can be done incrementally starting with TLS material (EdgionTls, BackendTLS,
outbound TLS, health-check TLS).

## Notes

- Related but distinct: `tasks/improve-issues/arch-01-conf-sync-broadcasts-all-secrets-to-every-gateway.md`
  (who receives secrets); this item is about how secret material is modeled inside resource schemas.
- `skills/01-architecture/05-resources/00-schema-transport-contract.md` defines the current embedding rules
  (classification table); it is a mandatory contract for today's model, not a decision rejecting a separate
  secret transport.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P3 unchanged (direction).
- `common/secret_slot.rs:10-24`: `SecretSlot` holds the full `k8s_openapi::api::core::v1::Secret`.
- `credentials/materials.rs:10-30` documents that the embedded `Secret` copies are serialized through serde_json and gRPC to the Gateway and are never wiped.
- This is related to arch-01 (who receives secrets) but distinct: this finding is the schema-level modelling, and it is not a duplicate.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `common/secret_slot.rs`, `credentials/`, `redact.rs`, `health_check.rs`, `backend_tls_policy.rs` and `edgion_tls.rs` are unchanged; whole `Secret` objects are still embedded.
- `gateway.rs` and `common/outbound_tls.rs` changed only schema-pattern constants (line shifts of -3); refs updated to `gateway.rs:282,431` and `outbound_tls.rs:99-109`.
