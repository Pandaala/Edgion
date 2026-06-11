---
name: resource-link-sys
description: LinkSys resource — external system connector definition, multiple Provider types, Gateway-side client management, unified secretRef credential resolution.
---

# LinkSys resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)
> **Credential resolution layer**: see the "Secret → Material resolution layer" section in [10-secret.md](10-secret.md)
> **External contract / YAML schema**: see [../../02-features/03-resources/18-link-sys.md](../../02-features/03-resources/18-link-sys.md)

LinkSys defines external system connectors (Redis / Etcd / Elasticsearch / Webhook / Kafka), used by AccessLog, rate-limiting plugins, Webhook Key resolution, and similar features. All credentials reference K8s `Secret`s via `secretRef`; **the CRD does not accept any inline sensitive fields** (hard-rejected by `deny_unknown_fields`).

## Source code locations

| Purpose | Path |
|------|------|
| Controller Handler | `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs` |
| Credential pipeline (Controller) | `src/core/controller/conf_mgr/sync_runtime/resource_processor/credential/` |
| Resource type definitions | `edgion-resources/src/resources/link_sys/` (mod.rs, redis.rs, etcd.rs, elasticsearch.rs, webhook.rs, common.rs) |
| Shared SecretObjectReference / SecretSlot / TLS types | `edgion-resources/src/resources/common/{secret_object_ref,secret_slot,tls}.rs` |
| Credential resolution layer (Controller + Gateway shared) | `edgion-resources/src/credentials/` (materials / resolver / resolvers) |
| Gateway credential four-state | `src/core/gateway/link_sys/credential.rs` |
| Gateway TLS assembly | `src/core/gateway/link_sys/tls.rs` |
| Gateway runtime Store | `src/core/gateway/link_sys/runtime/store.rs` |
| Provider clients | `src/core/gateway/link_sys/providers/{redis,etcd,elasticsearch}/` |

## Recommended `use` paths (avoiding name confusion)

The LinkSys CRD and the log-output / rotation configuration (used by link_sys runtime to write logs) have similar names but are two completely different sets of types:

| What you want | Recommended `use` path |
|------|------|
| LinkSys CRD structs (Redis/Etcd/ES/Webhook connection definitions) | `use crate::types::{LinkSys, LinkSysSpec};` (top-level re-export)<br>or `use crate::types::resources::link_sys::*;` (direct path) |
| Log output / file rotation configuration (`StringOutput`, `RotationConfig`, `LocalFileWriterCfg`) | `use crate::types::output::*;` (**the only path**; the previous `crate::types::link_sys` alias has been removed) |
| link_sys runtime (`DataSender` trait, provider clients) | `use crate::core::gateway::link_sys::*;` |

The module path `crate::types::link_sys` **no longer exists**. If you still see this notation, it is a residual legacy alias — change it to one of the paths above.

## CRD top-level layout

`LinkSysSpec` is a tagged enum with `#[serde(tag = "type", content = "config")]`:
- The top level has only two fields: `spec.type` and `spec.config`
- `config`'s type is determined by `type`: `redis` → `RedisClientConfig`, `etcd` → `EtcdClientConfig`, `elasticsearch` → `ElasticsearchClientConfig`, `webhook` → `WebhookServiceConfig`, `kafka` → `KafkaClientConfig` (placeholder)
- The legacy nested `spec.config.type` / `spec.config.config` form is no longer accepted

`SecretObjectReference` (`edgion-resources/src/resources/common/secret_object_ref.rs`), `SecretSlot` (`edgion-resources/src/resources/common/secret_slot.rs`), and `TlsConfig` / `TlsCerts` (`edgion-resources/src/resources/common/tls.rs`) have been extracted from each provider module into a shared common module to avoid multiple near-duplicate copies. `SecretObjectReference` carries `group / kind / name / namespace` (the first two are optional, defaulting to `core/Secret`), reserving room for non-Secret credential sources (Vault / ExternalSecret) in the future. `SecretSlot` is the `{ secret_ref: SecretObjectReference, secret: Option<Secret> }` slot, embedded into each `*Auth` via `#[serde(flatten)]`, with no change in YAML form. The LinkSys subcrates keep stable paths via `pub use`.

## Controller-side processing

### preparse / validate

- Calls `LinkSys::validate_config()`:
  - Redis/Etcd/Elasticsearch with empty `endpoints` → error
  - Redis with `topology.mode=sentinel` but no `sentinel` sub-object / `mode=cluster` but no `cluster` sub-object → error
  - Webhook: invalid URI, invalid method, `statusOnError` out of range, `maxResponseBytes` over the cap → error
- Unimplemented providers (e.g., `Kafka`) **do not** produce errors and stay `Accepted=True` (the schema is valid; the backend is simply not yet wired up — this avoids flipping existing resources to Invalid on upgrade).

### parse — credential resolution

Unlike the older "pass through directly", the new parse goes through the full **credential pipeline** (`resource_processor/credential/pipeline.rs`):

1. Via `AuthSpec` / `TlsSpec` traits + the generic `drive_standard`, walks every `SecretObjectReference` in the spec uniformly:
   - The `AuthSpec` trait binds "declaration → resolver selection → material extraction" to a concrete `*Auth` type (Redis/Etcd's `RedisAuth`/`EtcdAuth`; ES/Webhook's tagged-enum variants)
   - The `TlsSpec` trait handles each provider's optional `tls.certs.secretRef` (`TlsConfig` already implements this trait, reusable directly)
   - `resolve_for_spec` calls `drive_standard::<A: AuthSpec, T: TlsSpec>` to fold over all ref handling
   - Adding a new backend just needs: (1) impl `AuthSpec`; (2) add a `SystemConfig` variant; (3) add one line of `drive_standard(...)` in `resolve_for_spec`; (4) if it has TLS, `TlsConfig` already impls `TlsSpec` and can be reused directly
2. For each ref, run the seven-step pipeline:
   1. Compute the effective namespace (default = LinkSys's namespace)
   2. Register the `owner→secret` dependency in `SecretRefManager` (register even if subsequent steps fail, so that when the Secret appears/changes a self-healing requeue is possible)
   3. Cross-namespace goes through `ReferenceGrantStore` for authorization
   4. Look up the `Secret` in `runtime.secret_store`
   5. Validate `Secret.type` is in the resolver's allowlist
   6. Extract `Secret.data`
   7. Call the corresponding `SecretMaterialResolver::parse` for missing-key / decoding validation (Material is discarded; the Gateway recomputes it)
3. On success, write the **entire `k8s_openapi::Secret` object** back into the spec's `auth.secret` / `tls.certs.secret` field (not just data; type / metadata is preserved so the Gateway can perform the secondary `allowed_secret_types` check). These fields have `skip_serializing_if = "Option::is_none"` in the CRD schema, so they do not appear in user YAML, while still being serialized to the Gateway via conf_sync's `serde_json`.
4. Pipeline three-state `ResolveOutcome` (applies only to **the current ref** and does not stop other refs in the same LinkSys from continuing the pipeline — see "ResolvedRefs multi-error collection" in [10-secret.md](10-secret.md)):
   - `Ok(Secret)` → embed Secret into spec
   - `Pending(SecretNotFound | ReferenceGrantMissing)` → leave `secret: None` on the current ref, accumulate errors; auto-requeue once the Secret / Grant arrives
   - `Invalid(MissingKeys | DecodeError | InvalidSecretType)` → leave `secret: None` on the current ref, accumulate errors; wait for an operator to fix the Secret content
   - Regardless of which branch is hit, `resolve_for_spec` continues processing the next ref (auth failure does not short-circuit tls, and vice versa); all results are written into `ResolvedRefs.message` together; `reason` is selected as the most severe according to the `pick_resolved_refs_reason` priority table.

### update_status

Handler maintains two Gateway-API-style conditions in tandem:

| Condition | True | False reasons |
|-----------|------|----------------|
| `Accepted` | spec is valid + `validate_config()` passes | `Invalid` (validation error list) |
| `ResolvedRefs` | All secretRef / tls.secretRef are `Ok` | `SecretNotFound` / `RefNotPermitted` / `InvalidSecretType` / `MissingKeys` / `DecodeError` |

`SecretNotFound` / `RefNotPermitted` are **Pending** semantics (await self-heal); the rest are `Invalid` semantics (await human fix). The Controller automatically requeues based on change events from `SecretRefManager` / `ReferenceGrantStore`; the Handler does not need to retry manually.

`kubectl get linksys` output has five columns: `Type` / `Endpoints` / `Accepted` / `ResolvedRefs` / `Age` (see CRD `additionalPrinterColumns`).

## Gateway-side processing

### When conf_sync delivers the spec

`LinkSysStore` manages the provider client lifecycle:
- LinkSys created → build the provider client
- LinkSys updated → consult `config_mapping` to decide whether the client must be rebuilt (Redis params changed, endpoints changed, auth changed, tls changed → rebuild; observability changed → only update hot fields)
- LinkSys deleted → gracefully close connections / return them to the pool

### Credential second-stage resolution: `CredentialState`

In the spec the Gateway receives, `auth.secret` / `tls.certs.secret` already contain the embedded `Secret` object. The `core::gateway::link_sys::credential` module abstracts credentials into four states:

```rust
enum CredentialState<M> {
    NotRequired,     // CRD has no auth at all → provider uses anonymous connection
    Pending,         // secretRef configured but Controller has not embedded the secret yet
    Invalid(reason), // secret embedded but resolver.parse failed
    Resolved(M),     // usable Material
}
```

Constraints:
- `Debug` is **redacted**: the `Resolved` variant does not print Material; this way `tracing::info!(state=?state)` never leaks passwords.
- `is_buildable()` returns true only for `NotRequired` / `Resolved`. The provider's dispatch path is fail-fast: `Pending` / `Invalid` `bail!` directly, and **never** degrades to "empty credentials / anonymous connection" (a historical silent-degrade bug).
- For tagged-enum auth (ES / Webhook), wrap with the generic `CredentialState<ResolvedCredential>`, with `ResolvedCredential` further dispatching by variant.

### TLS assembly

`src/core/gateway/link_sys/tls.rs` plugs `TlsConfig` + the resolved `TlsMaterial` into each provider's TLS configuration:
- `ca_pem`: if `TlsCerts.caCert` inline is present, use the inline value; otherwise use the material's `ca_pem`.
- `client_cert_pem` / `client_key_pem`: prefer material (via secretRef); the private key is `Zeroizing<Vec<u8>>`.
- `insecureSkipVerify` is handled per provider capability: etcd ignores and warns; Redis/ES/Webhook honor it.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| LinkSys → Secret | `Secret` | `SecretRefManager` (auth + tls) | Secret change/delete → cascade requeue this LinkSys |
| LinkSys → ReferenceGrant | `ReferenceGrant` | `ReferenceGrantStore` | Cross-ns secretRef authorization; Grant change → cascade requeue |
| LinkSys ← AccessLog | AccessLog config | reference by `namespace/name` | ES / Redis / Webhook as log backends |
| LinkSys ← EdgionPlugins | plugin config | reference by `namespace/name` | Redis distributed rate limiting, Webhook Key resolution |

The full internal-dependency and requeue cascade graph is in [19-resource-dependency.md](19-resource-dependency.md).

## Related tasks / design gaps

- The Zeroize scope boundary (why end-to-end zeroization is not done when the embedded Secret travels through `serde_json` / gRPC) is in the "Zeroize scope" section of [10-secret.md](10-secret.md).
- Provider client rebuild strategies are in `providers/*/config_mapping.rs`.
