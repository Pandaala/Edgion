---
name: resource-secret
description: Secret resource — runtime-owned Controller storage, default no-sync behavior, cascading requeue, and shared Secret-to-Material resolution.
---

# Secret resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

Secret is a Kubernetes core resource and a default **no_sync_kind**. It is not sent as an
independent Gateway cache. Each ConfCenter's `ControllerRuntime.secret_store` provides Secret data
to handlers, and its `SecretRefManager` cascades requeues to dependents. Sensitive data needed by
the Gateway is embedded into the resolved resource and sent through conf_sync. EdgionAcme account
and DNS credentials are instead consumed by the Kubernetes-only Controller AcmeService.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/secret.rs`
- Credential resolution layer (Controller + Gateway shared): `edgion-resources/src/credentials/`
- Controller-side credential pipeline: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/`
- Gateway-side credential four-state: `edgion-common/src/link_sys_core/credential.rs`

## Controller-side processing

### parse

1. **Update the runtime store** through `ctx.runtime().secret_store.update(...)`; other handlers
   read the same runtime instance.
2. **Initialization-stage accumulation**: collect Secrets during the initial List; on init_done,
   `secret_store.replace_all(...)` performs the authoritative replacement.
3. **Consumer-owned material validation**: dependency edges identify resources to
   requeue, not credential types. The generic handler does not infer OIDC from an
   EdgionPlugins edge. Each bound-field resolver owns its key, UTF-8 and emptiness
   rules; BasicAuth and OIDC can share the same lifecycle without sharing material
   contracts (ARCH-157).

### on_init_done / on_change / on_delete

- `on_init_done`: perform `secret_store.replace_all(...)` for an authoritative replacement.
- `on_change`: via `SecretRefManager`, find all resources referencing this Secret and requeue them one by one.
- `on_delete`: remove from `runtime.secret_store` + cascading requeue.

## Runtime-owned SecretStore

`ControllerRuntime` owns one SecretStore per ConfCenter lifecycle. Handlers access it through
`HandlerContext::runtime()`. `ControllerRuntime::reset()` clears reload-scoped Secret state,
preventing data from leaking between center generations.

## Secret → Material resolution layer

The `SecretMaterialResolver` trait under `edgion-resources/src/credentials/` maps `Secret.data` (`BTreeMap<String, ByteString>`) into a strongly-typed `Material`. Controller and Gateway **share the same set of resolvers**, avoiding duplicated implementations on either end.

### trait

```rust
pub trait SecretMaterialResolver: Send + Sync {
    type Material: Send + Sync;
    fn required_keys(&self) -> &'static [&'static str];
    fn allowed_secret_types(&self) -> &'static [&'static str] { &["Opaque", ""] }   // default
    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError>;
}
```

`ResolveError`: `MissingKeys(Vec<&'static str>)` (all missing keys reported at once) / `DecodeError { key, source }` / `InvalidSecretType { got, allowed }` (Secret.type not in the allowlist, validated by the pipeline).

### Resolver table

| Resolver | required_keys (Secret.data) | allowed_secret_types | Material |
|----------|------------------------------|----------------------|----------|
| `BasicAuthResolver` | `password` required, `username` optional | `kubernetes.io/basic-auth`, `Opaque`, `""` | `BasicAuthMaterial` |
| `BearerResolver` | `bearerToken` | `Opaque`, `""` | `BearerMaterial` |
| `EsApiKeyResolver` | `apiKeyId` + `apiKeySecret` | `Opaque`, `""` | `EsApiKeyMaterial` |
| `AcmeAccountResolver` | `accountCredentials` (JSON) | `Opaque`, `""` | `AcmeAccountMaterial` |
| `TlsCertsResolver` | at least one of `tls.crt` / `tls.key` / `ca.crt` (all three are listed in `required_keys()` so status can print the full candidate set) | `kubernetes.io/tls`, `Opaque`, `""` | `TlsMaterial` |

Application-layer keys use camelCase; TLS keys follow the `kubernetes.io/tls` standard.

Webhook credentials use request-template Secret references, resolved by the Controller
into `config.resolvedSecrets`; they do not use a separate auth enum or credential
material parser. See the [Webhook flow](../03-controller-gateway-link/webhook.md).
ARCH-051 retired the orphan Webhook auth types and dedicated parsers; live HmacAuth
retains its independent grouped credential resolver.

### Which resources go through these resolvers

| Resource | Field | Resolver |
|------|------|----------|
| `LinkSys` (Redis / Etcd) | `spec.config.auth.secretRef` | `BasicAuthResolver` |
| `LinkSys` (Elasticsearch) | `spec.config.auth` (tagged enum) | `BasicAuth` / `EsApiKey` / `Bearer` |
| `LinkSys` (all providers) | `spec.config.tls.certs.secretRef` | `TlsCertsResolver` |
| `EdgionAcme` | `spec.privateKeySecretRef` | `AcmeAccountResolver` |

> The `*Auth` / `TlsCerts` of LinkSys / EdgionAcme have `deny_unknown_fields` enabled; inlining sensitive fields such as `username` / `password` / `bearerToken` / `apiKeyId` / `clientKey` directly into the CRD will be rejected during YAML deserialization. `caCert` / `clientCert` are public material and may still be inlined.

### Controller → Gateway responsibility split

```
┌─────────────────────────────┐   conf_sync (gRPC)   ┌──────────────────────────────┐
│ Controller                  │ ─────────────────►  │ Gateway                      │
│ 1. SecretRefManager register│                     │ 4. From the embedded         │
│ 2. pipeline.resolve_secret: │                     │    Secret.data, call         │
│    ReferenceGrant check →   │                     │    resolver.parse again to   │
│    SecretStore lookup →     │                     │    get the Material and      │
│    resolver.parse (validate │                     │    wrap it in                │
│    only) → OK → embed full  │                     │    CredentialState<M>        │
│    Secret into              │                     │ 5. Resolved/NotRequired →    │
│    *.auth.secret /          │                     │    build provider; Pending/  │
│    tls.secret               │                     │    Invalid → fail-fast bail! │
│ 3. Write Accepted +         │                     │    (does NOT degrade to an   │
│    ResolvedRefs             │                     │    anonymous connection)     │
└─────────────────────────────┘                     └──────────────────────────────┘
```

- **Controller** (`resource_processor::credential::pipeline`): namespace + `ReferenceGrant` authorization, `SecretStore` lookup, `resolver.parse` performs missing-key / decoding / Secret.type validation (Material is discarded). The three-state `ResolveOutcome` maps to the `ResolvedRefs` condition: `Ok`→True; `Pending`→False/`SecretNotFound`/`RefNotPermitted` (await requeue); `Invalid`→False/`MissingKeys`/`DecodeError`/`InvalidSecretType` (operator must fix the Secret).
- **Gateway** (`core::gateway::link_sys::credential`): from the embedded `Secret` in the spec, compute Material once more. `CredentialState<M>` has four states `NotRequired` / `Pending` / `Invalid` / `Resolved`; `Debug` redacts the `Resolved` variant to avoid leaking passwords via `tracing::info!(state=?)`. Dispatch builds a client only in the `NotRequired` / `Resolved` states; it **does not** degrade to an anonymous-credentials connection (a historical bug).

### ResolvedRefs multi-error collection (mandatory rule)

> Applies to: any Handler that resolves **multiple** Secret / resource refs in a single parse.
> Currently in place: `LinkSysHandler`, `EdgionPluginsHandler`, the HTTPRoute family (via `route_utils::collect_ref_not_permitted_errors`).

**Rule**: when a Handler resolves multiple refs, it must collect all failures into `Vec<ResolvedRefsError>` and feed them to `resolved_refs_condition` in one go. It **must not `return` on the first `Err`**.

```rust
async fn resolve_for_spec(...) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    if let Some(auth) = &mut r.auth {
        if let Err(e) = resolve_one(ctx, owner, &auth.secret_ref, &BasicAuthResolver).await {
            errors.push(e);
            // Do NOT `return errors;` — continue to tls
        } else { /* embed Secret */ }
    }
    if let Some(e) = resolve_tls(ctx, owner, r.tls.as_mut()).await {
        errors.push(e);
    }
    errors
}
```

**Why mandatory**:

1. **Gateway API message contract**: `status.conditions[ResolvedRefs].message` is an aggregated message. When a single LinkSys has both auth and tls Secrets misconfigured, the operator must see all errors in one reconcile, otherwise "fix auth, reconcile, find tls is still wrong, fix again" wastes another round-trip.
2. **`pick_resolved_refs_reason` priority table only works this way**: `status_utils::pick_resolved_refs_reason` orders reasons by severity (`InvalidKind` > `RefNotPermitted` > `InvalidSecretType` > `EdgionConfigDataNotFound` > `BackendNotFound` > `SecretNotFound` > `DecodeError` > `MissingKeys`). Fail-fast emits only one error each time, making the priority table useless.
3. **Half-finished spec is safe**: failure branches leave `auth.secret: None` / `tls.certs.secret: None`, which will not be consumed because the Gateway side only `build_link_sys_instance_from_status` → builds a client when `ResolvedRefs=True` (see I4/I5, the four-state `CredentialState`).

**When adding a new Handler / new field**: any function shaped like `resolve_for_spec` that resolves refs in multiple places should add a `test_<foo>_and_<bar>_both_fail_collects_both_errors` unit test asserting `errors.len() == 2`, the picked reason is correct, and the message contains both segments — preventing future contributors from accidentally reverting to fail-fast. Historical reference: `tasks/fix-change/M7-linksys-multi-error-collect.md` (the full change moving LinkSys away from fail-fast).

### Zeroize scope (defense-in-depth, not end-to-end)

Only Material fields that **hold raw private keys** are wrapped in `Zeroizing`:
- `AcmeAccountMaterial.credentials` (`Zeroizing<String>`)
- `TlsMaterial.client_key_pem` (`Option<Zeroizing<Vec<u8>>>`)
- Application credentials (`BasicAuthMaterial`, `BearerMaterial`, `EsApiKeyMaterial`,
  and `AiApiKeyMaterial`) use owned `String` values; their Debug output is redacted
  independently of zeroization.

The following plaintext copies are **not** zeroized: `SecretStore` cache, `serde_json` serialization buffer, conf_sync gRPC transport, Gateway runtime store cache, the `Value` tree from `serde_json::from_str::<Value>` inside `AcmeAccountResolver::parse`, intermediate Strings such as `json.to_string()`. The upstream K8s apiserver response buffer itself cannot be zeroized; the marginal value of end-to-end zeroization is limited, so this layer guarantees only "the final material container is zeroized". For details, see the `materials.rs` module-header caveat and the NOTE in `resolvers/acme.rs`.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| Secret → Gateway | Gateway | SecretRefManager | Listener `certificateRefs` and Gateway-level backend `clientCertificateRef` |
| Secret → EdgionTls | EdgionTls | SecretRefManager | `secret_ref` / `ca_secret_ref` |
| Secret → EdgionPlugins | EdgionPlugins | SecretRefManager | JWT / BasicAuth / HMAC / KeyAuth / OIDC, etc. |
| Secret → EdgionAcme | EdgionAcme | SecretRefManager | `privateKeySecretRef` (account private key, `AcmeAccountResolver`) |
| Secret → LinkSys | LinkSys | SecretRefManager | Provider auth references (Redis/Etcd basic, Elasticsearch basic/apiKey/bearer, OTLP bearer), Kafka `sasl.password.secretRef`, enabled TLS CA/client-certificate references, and Webhook request-template Secret references |
| Secret → BackendTLSPolicy | BackendTLSPolicy | SecretRefManager | Backend TLS CA and client certificate |
