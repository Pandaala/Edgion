---
name: resource-link-sys
description: LinkSys resource — external system connector definition, multiple Provider types, Gateway-side client management, unified secretRef credential resolution.
---

# LinkSys resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)
> **Credential resolution layer**: see the "Secret → Material resolution layer" section in [10-secret.md](10-secret.md)
> **External contract / YAML schema**: see [../../02-features/03-resources/18-link-sys.md](../../02-features/03-resources/18-link-sys.md)

LinkSys defines Redis, Etcd, Elasticsearch, Webhook, Kafka, OTLP, HttpDns, and
credentialSource configuration. It is used by access logging, rate limiting,
Webhook key resolution, DNS resolution, CredentialInjector, and related features.

## Open Extension Types

`spec.type` is an open CRD string. Known variants keep their typed validation,
including strict credential substructures and nested enums. Unknown variants decode
as a schema-hidden `Unsupported` unit marker; their name and payload are discarded.
Controller reports `Accepted=False/Invalid` and still publishes the replacement.
The normalized marker serializes with an empty `config: {}` to preserve the required
CRD envelope. It survives conf_sync and cannot initialize a provider. Admin writes
persist this normalized value; after upgrading, reapply the original supported
configuration to recover its discarded type and payload.

On replacement, Controller clears obsolete Secret/cross-namespace edges and
credential-source desired state/results, then requeues dependents. Gateway full and
partial updates retire any old provider through the existing wrong-kind removal.
HTTPDNS references resolve against the current raw LinkSys and retain their existing
System-resolver fallback on a missing or wrong-type target. Existing borrowed handles
and asynchronous cleanup follow normal deletion semantics. Reapplying a supported
type allows recovery. Adding a provider requires Rust/runtime wiring and tests,
not an extension-name CRD whitelist update.

## Source code locations

| Purpose | Path |
|------|------|
| Controller Handler | `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs` |
| Credential-source desired state and consumer registry | `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential_source_state.rs` |
| CredentialInjector LinkSys/Secret resolver | `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/credential_injector.rs` |
| Credential manager worker and publication lifecycle | `edgion-controller/src/services/credential_manager/service.rs` |
| Controller-only generated-credential memory store | `edgion-controller/src/services/credential_manager/memory.rs` |
| Kubernetes / FS-etcd worker startup | `edgion-controller/src/conf_mgr/conf_center/{kubernetes/center,non_kube_common}.rs` |
| Credential pipeline (Controller) | `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/` |
| Resource type definitions | `edgion-resources/src/resources/link_sys/` |
| Shared SecretObjectReference / SecretSlot / TLS types | `edgion-resources/src/resources/common/{secret_object_ref,secret_slot,tls}.rs` |
| Credential resolution layer (Controller + Gateway shared) | `edgion-resources/src/credentials/` (materials / resolver / resolvers) |
| Gateway credential four-state | `edgion-common/src/link_sys_core/credential.rs` |
| Gateway TLS assembly | `edgion-common/src/link_sys_core/tls.rs` |
| Gateway runtime Store | `edgion-gateway/src/link_sys/runtime/store.rs` |
| Provider clients | Gateway providers under `edgion-gateway/src/link_sys/providers/`; shared Etcd under `edgion-common/src/link_sys_core/providers/etcd/` |

## Recommended `use` paths (avoiding name confusion)

The LinkSys CRD and the log-output / rotation configuration (used by link_sys runtime to write logs) have similar names but are two completely different sets of types:

| What you want | Recommended `use` path |
|------|------|
| LinkSys resource structs | `edgion_resources::resources::link_sys::*` |
| Log output / file rotation configuration | `edgion_resources::resources::output::*` |
| Gateway LinkSys runtime | `crate::link_sys::*` inside `edgion-gateway` |

## CRD top-level layout

`LinkSysSpec` is a tagged enum with `#[serde(tag = "type", content = "config")]`:
- The top level has only two fields: `spec.type` and `spec.config`
- `config` is selected by `type`: Redis, Etcd, Elasticsearch, Webhook, Kafka, OTLP, HttpDns, or CredentialSource
- The legacy nested `spec.config.type` / `spec.config.config` form is no longer accepted

`SecretObjectReference` (`edgion-resources/src/resources/common/secret_object_ref.rs`), `SecretSlot` (`edgion-resources/src/resources/common/secret_slot.rs`), and `TlsConfig` / `TlsCerts` (`edgion-resources/src/resources/common/tls.rs`) have been extracted from each provider module into a shared common module to avoid multiple near-duplicate copies. `SecretObjectReference` carries `group / kind / name / namespace` (the first two are optional, defaulting to `core/Secret`), reserving room for non-Secret credential sources (Vault / ExternalSecret) in the future. `SecretSlot` is the `{ secret_ref: SecretObjectReference, secret: Option<Secret> }` slot, embedded into each `*Auth` via `#[serde(flatten)]`, with no change in YAML form. The LinkSys subcrates keep stable paths via `pub use`.

## Controller-side processing

### preparse / validate

- Calls `LinkSys::validate_config()`:
  - Redis validates its complete endpoint contract before publication: standalone has exactly one top-level endpoint, cluster has at least one, and sentinel has no top-level endpoints plus a non-empty `topology.sentinel.sentinels` list and `masterName`. Every active endpoint must parse, inactive topology blocks are rejected, and mixed TLS/plaintext schemes require `tls.enabled=true`. Validation errors identify only the field/index and never echo endpoint input.
  - Elasticsearch with empty `endpoints` → error
  - Etcd uses canonical endpoint, message-size, version-floor, keepalive and
    observability validation; Admin `/validate/LinkSys` calls the same pure rules.
    Gateway fingerprint/build and Common construction independently reject invalid
    policy, regardless of advisory Controller status.
  - Webhook (redesigned): a Service target (`target.name`/`target.port`) missing `name`/`port` or with an unsupported `kind`, OR a `url` target that is not origin-only, is set together with any Service field (mutually exclusive), or combines `http://` with `tls.enabled: true`; `request` template parse error; a field name in both `origin` and `custom`, duplicate names within a block, or a `headers.origin: [{name: Cookie}]` / non-empty `cookies` conflict; `statusOnError` out of range; `maxResponseBytes` over the cap → error. See [03-controller-gateway-link/webhook.md](../03-controller-gateway-link/webhook.md).
- Kafka has a live Gateway runtime client/store. HttpDns is consumed by the DNS resolver path and
  intentionally has no generic LinkSys runtime-client store.
- `credentialSource` is Controller-only acquisition metadata. Persist-on writes a generated
  Secret; `publication.persist: false` stores a capped map on
  `ControllerRuntime.credential_memory` (never on the LinkSys object). Kubernetes
  `all-serve` + persist-off is `Accepted=False` / `UnsupportedHaMode`. The Gateway
  `LinkSysStore` is a no-op for this variant. The active token still rides EdgionPlugins
  `resolvedCredential`.
- Persist-on outside Kubernetes is `Accepted=False` /
  `UnsupportedConfigurationCenter`. Persist-off is allowed on FS/etcd and uses
  the kube-less credential manager worker; that worker acquires credentials but
  performs no generated-Secret IO.

### parse — credential resolution

`TlsSpec::resolve` first clears `resolved_ca_certificates` and
`resolved_client_certificate`, then returns immediately for `enabled: false`.
Since `parse` clears the owner's previous Secret dependencies before rebuilding
them, disabling TLS also removes its old dependency edges. SAN/hostname admission
and `credential::tls_ready` apply only to enabled TLS blocks. The credential-source
resolver likewise excludes disabled TLS from its manager config and input revision
list. Auth references continue through their independent resolution path.

Unlike the older "pass through directly", the new parse goes through the full **credential pipeline** (`resource_processor/credential/pipeline.rs`):

1. Via `AuthSpec` / `TlsSpec` traits + the generic `drive_standard`, walks every `SecretObjectReference` in the spec uniformly:
   - The `AuthSpec` trait binds "declaration → resolver selection → material extraction" to a concrete `*Auth` type (Redis/Etcd's `RedisAuth`/`EtcdAuth`; Elasticsearch's tagged-enum variants). **Webhook no longer participates here:** its handler resolves only TLS; outbound credentials are `request.headers.custom` entries using `template: "... ${secretRef:ns/name:key} ..."` — the handler scans `collect_secret_ref_names`, fetches each Secret via `fetch_secret` (ReferenceGrant enforced for cross-namespace), and embeds resolved values in `spec.resolved_secrets`, which rides the normal conf_sync path to the gateway. The ignored top-level `auth` field still present in code is removal debt, not a supported compatibility contract; see [webhook.md](../03-controller-gateway-link/webhook.md).
   - The `TlsSpec` trait handles enabled `TlsConfig` CA and client-certificate references (`validation.caCertificateRefs` and `clientCertificateRef`); disabled blocks contribute no references.
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
3. On success, write the resolved Secret into the transport slot. `#[schemars(skip)]` keeps the
   field out of the CRD, serde keeps it in conf_sync, and the redaction adapter hides it from Admin
   config output. `skip_serializing_if` only omits `None`; it is not a schema control. See
   [00-schema-transport-contract.md](00-schema-transport-contract.md).
4. Pipeline three-state `ResolveOutcome` (applies only to **the current ref** and does not stop other refs in the same LinkSys from continuing the pipeline — see "ResolvedRefs multi-error collection" in [10-secret.md](10-secret.md)):
   - `Ok(Secret)` → embed Secret into spec
   - `Pending(SecretNotFound | ReferenceGrantMissing)` → leave `secret: None` on the current ref, accumulate errors; auto-requeue once the Secret / Grant arrives
   - `Invalid(MissingKeys | DecodeError | InvalidSecretType)` → leave `secret: None` on the current ref, accumulate errors; wait for an operator to fix the Secret content
   - Regardless of which branch is hit, `resolve_for_spec` continues processing the next ref (auth failure does not short-circuit tls, and vice versa); all results are written into `ResolvedRefs.message` together; `reason` is selected as the most severe according to the `pick_resolved_refs_reason` priority table.

### update_status

Handler maintains two Gateway-API-style validity/reference conditions plus one independent
advisory condition:

| Condition | True | False reasons |
|-----------|------|----------------|
| `Accepted` | spec is valid + `validate_config()` passes | `Invalid` (validation error list); `UnsupportedConfigurationCenter` (persist-on outside Kubernetes); `UnsupportedHaMode` (persist-off without a single publisher) |
| `ResolvedRefs` | All secretRef / tls.secretRef are `Ok` | `SecretNotFound` / `RefNotPermitted` / `InvalidSecretType` / `MissingKeys` / `DecodeError` |

`UnknownConfigFields=True/UnknownFieldsDetected` is added when the operator-authored document
contains a key that the generated typed LinkSys schema does not recognize. It is removed after a
clean reapply and never changes `Accepted`, `ResolvedRefs`, credential resolution, or conf_sync.
The detector covers all eight `SystemConfig` variants. Declared map values and arbitrary-JSON values remain
open boundaries, while unknown siblings are still reported. The same findings are available through
the Admin write warning, read-only validate endpoint, fleet diagnostics endpoint, and the bounded
per-kind Controller metric.

`SecretNotFound` / `RefNotPermitted` are **Pending** semantics (await self-heal); the rest are `Invalid` semantics (await human fix). The Controller automatically requeues based on change events from `SecretRefManager` / `ReferenceGrantStore`; the Handler does not need to retry manually.

`kubectl get linksys` output has five columns: `Type` / `Endpoints` / `Accepted` / `ResolvedRefs` / `Age` (see CRD `additionalPrinterColumns`).

### Credential manager lifecycle

The serial LinkSys and EdgionPlugins workqueue handlers only validate, register
desired sources/consumers, resolve snapshots, and enqueue dependency reprocessing.
They never call the issuer and never wait for credential acquisition or Secret
publication. The separately started CredentialManager worker owns issuer calls,
rotation timers, bounded concurrency, memory publication, and Kubernetes Secret
IO.

Publication and teardown invariants:

- `true` to `false` publishes or reacquires a live memory entry before deleting
  owned generated Secrets. Fenced delete failures remain in `state.publications`
  and are retried.
- `false` to `true` retains the memory slot until
  `published_consumer_count == desired_consumer_count`.
- Removing an EdgionPlugins consumer removes its desired publication. Removing
  the last consumer also clears the source memory slot.
- Leadership loss, runtime reset, source delete or type change, source UID
  replacement, last-consumer removal, and wipe-all clear memory.
- Source identity and publication fences are rechecked before a memory write or
  Secret mutation. A stale reconcile result cannot publish into a replacement
  source generation.

## Gateway-side processing

### When conf_sync delivers the spec

`LinkSysStore` manages the provider client lifecycle:
- LinkSys created → build the provider client
- LinkSys updated → consult `config_mapping` to decide whether the client must be rebuilt (Redis params changed, endpoints changed, auth changed, or TLS changed → rebuild)
- LinkSys deleted → remove current lookup and signal provider-owned cleanup without waiting for every task or external handle
- `type: credentialSource` is a no-op in `LinkSysStore` (no runtime client). The
  Gateway never contacts the issuer; the active token arrives on EdgionPlugins
  `resolvedCredential`.

### Credential second-stage resolution: `CredentialState`

In the resolved spec, transport Secret slots contain the embedded object. The
`edgion_common::link_sys_core::credential` module abstracts credentials into four states:

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
- For tagged-enum auth (Elasticsearch), wrap with the generic `CredentialState<ResolvedCredential>`, with `ResolvedCredential` further dispatching by variant. (Webhook is no longer a tagged-enum-auth provider — its outbound credentials are per-field `request.headers.custom` secret sources resolved in the webhook provider, not via `CredentialState`.)

### TLS assembly

The Webhook provider removes disabled `conn.tls` before calling
`ManagedOutboundClient::from_parts`. The shared engine uses block presence to
select the Service target's scheme and construct its HTTP client. This boundary
keeps disabled LinkSys TLS inert for both business calls and active health checks,
while URL targets retain their explicit `http` or `https` scheme.

`edgion-common/src/link_sys_core/tls.rs` plugs `TlsConfig` + the resolved `TlsMaterial` into each provider's TLS configuration:
- `ca_pem`: derived from the resolved `validation.caCertificateRefs` Secret(s) carrying a `ca.crt` key. The old inline `TlsCerts.caCert` field has been removed. A custom CA replaces the public roots entirely.
- `client_cert_pem` / `client_key_pem`: from the Secret referenced by `clientCertificateRef`; the private key is `Zeroizing<Vec<u8>>`.
- `verify` is enforced per provider: Redis and Etcd always verify (`verify: false` is rejected at startup — the SDK has no skip-verify API); ES, Webhook, and other connectors honor `verify: false` as an operator decision (a WARN is emitted at config-load time). Trust anchors come from `validation.caCertificateRefs` (for LinkSys, only `kind: Secret` is supported in this release) or `validation.wellKnownCACertificates: System` (built-in webpki roots).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| LinkSys → Secret | `Secret` | `SecretRefManager` (auth + tls) | Secret change/delete → cascade requeue this LinkSys |
| LinkSys → ReferenceGrant | `ReferenceGrant` | `ReferenceGrantStore` | Cross-ns secretRef authorization; Grant change → cascade requeue |
| LinkSys ← AccessLog | AccessLog config | reference by `namespace/name` | Elasticsearch / Kafka / OTLP log backends; LocalFile is configured directly and is not LinkSys |
| LinkSys ← EdgionPlugins | plugin config | reference by `namespace/name` | Redis distributed rate limiting, Webhook Key resolution, CredentialInjector (`credentialSource`) |

The full internal-dependency and requeue cascade graph is in [19-resource-dependency.md](19-resource-dependency.md).

## Related tasks / design gaps

- The Zeroize scope boundary (why end-to-end zeroization is not done when the embedded Secret travels through `serde_json` / gRPC) is in the "Zeroize scope" section of [10-secret.md](10-secret.md).
- Provider client rebuild strategies are in `providers/*/config_mapping.rs`.

## OTLP resource path

`type: otlp` is the strict OTLP/gRPC origin/Bearer/TLS schema in
`edgion-resources/src/resources/link_sys/otlp.rs`. Controller uses drive_standard
with a bounded adapter delegating BearerResolver; SecretSlot and shared TlsConfig
own redacted resolved material. The pure validator rejects URI userinfo/path/query,
legacy inline fields, unsupported TLS policy and malformed reference identities.
Gateway preparation revalidates rather than relying on advisory Controller status.

TLS resolution visits every enabled CA/client reference even after an earlier
failure, so each unresolved dependency can trigger recovery. It publishes either
all resolved TLS material or none; disabling TLS clears material and old edges.
Auth and TLS resolution remain independent. Kubernetes, FS and etcd share this
pipeline, with existing backend-specific stores/status persistence.

LinkSys auth.secretRef declares all four operator fields (name, namespace, group,
kind) in the structural CRD subtree so Kubernetes cannot prune unsupported group
or kind before typed validation. The focused CRD guard compares this subtree to
SecretObjectReference. Handler apply/delete clear both Secret and cross-namespace
reference managers; failed references still rebuild their current wakeup edges.
