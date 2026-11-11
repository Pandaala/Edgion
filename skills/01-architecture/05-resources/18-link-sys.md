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
HTTPDNS references resolve against the current prepared typed runtime and retain
their existing System-resolver fallback on a missing or wrong-type target. A valid
HTTPDNS resource whose preparation failed retains its own configured fallback.
Existing borrowed handles
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

`SecretObjectReference` (`edgion-resources/src/resources/common/secret_object_ref.rs`), `SecretSlot` (`edgion-resources/src/resources/common/secret_slot.rs`), and `TlsConfig` / `TlsCerts` (`edgion-resources/src/resources/common/tls.rs`) have been extracted from each provider module into a shared common module to avoid multiple near-duplicate copies. `SecretObjectReference` carries `group / kind / name / namespace`: omitted group means the empty Kubernetes core group and omitted kind means `Secret`. Literal `core` is a finite Edgion custom-field convenience, not a native Gateway API or ACME default. The shared wire type already supports CredentialInjector's exact `edgion.io/LinkSys` branch; LinkSys provider auth/TLS fields retain their own Secret-only target contracts. `SecretSlot` is the `{ secret_ref: SecretObjectReference, secret: Option<Secret> }` slot, embedded into each `*Auth` via `#[serde(flatten)]`, with no change in YAML form. The LinkSys subcrates keep stable paths via `pub use`.

## Controller-side processing

### preparse / validate

- Calls `LinkSys::validate_config()`:
  - Redis validates its complete endpoint contract before publication: standalone has exactly one top-level endpoint, cluster has at least one, and sentinel has no top-level endpoints plus a non-empty `topology.sentinel.sentinels` list and `masterName`. Every active endpoint must parse, inactive topology blocks are rejected, and mixed TLS/plaintext schemes require `tls.enabled=true`. Validation errors identify only the field/index and never echo endpoint input.
  - Elasticsearch with empty `endpoints` → error. Its resources-owned
    `ElasticsearchClientConfig::validate_date_pattern` also rejects an explicit
    `index.datePattern` that cannot format a UTC timestamp, including parsing-only
    `%#z`. Gateway candidate preparation calls the same validator before
    fingerprint reuse or construction; Controller status alone is advisory.
  - Etcd validates endpoints, timeouts and keepalive through shared pure rules.
    Connection options use the upstream SDK;
    custom message limits, version admission, discovery and operation observations
    are outside the supported configuration.
  - Webhook (redesigned): a Service target (`target.name`/`target.port`) missing `name`/`port` or with an unsupported `kind`, OR a `url` target that is not origin-only, is set together with any Service field (mutually exclusive), or combines `http://` with `tls.enabled: true`; `request` template parse error; a field name in both `origin` and `custom`, duplicate names within a block, or a `headers.origin: [{name: Cookie}]` / non-empty `cookies` conflict; `statusOnError` out of range; `maxResponseBytes` over the cap → error. See [03-controller-gateway-link/webhook.md](../03-controller-gateway-link/webhook.md).
- Kafka has a live Gateway runtime client/store. HttpDns prepares an immutable
  Common resolver handle published in the Gateway's unified typed LinkSys runtime
  table; it has no persistent runner.
- `credentialSource` is Controller-only acquisition metadata. Persist-on writes a generated
  Secret; `publication.persist: false` stores one active entry per source identity on
  `ControllerRuntime.credential_memory` (never on the LinkSys object). Kubernetes
  `all-serve` + persist-off is `Accepted=False` / `UnsupportedHaMode`. The Gateway
  `LinkSysStore` is a no-op for this variant. The active token still rides EdgionPlugins
  `resolvedCredential`.
- Persist-on outside Kubernetes is `Accepted=False` /
  `UnsupportedConfigurationCenter`. Persist-off is allowed on FS/etcd and uses
  the kube-less credential manager worker; that worker acquires credentials but
  performs no generated-Secret IO.

### parse — credential resolution

LinkSys parsing first clears the owner's previous Secret/cross-namespace dependencies
and credential-source registry contribution, then binds `PreparedLinkSysReferences`
to the actual resource. `resolve_for_spec` clears old material before validating
its field references. These cleanup steps preserve delete/recreate coalescing;
they intentionally precede new acquisition and must not be treated as forbidden
side effects of an invalid incoming reference.

The bound credential pipeline (`resource_processor/credential/pipeline.rs`) then
handles actual provider fields:

1. `drive_standard<A: AuthSpec>` selects auth parsing from the actual provider
   variant and its bound auth token. `resolve_provider_tls` calls
   `auth_spec::resolve_tls` with the actual `LinkSysTlsReferences`; the removed
   `TlsSpec` trait is not an extension point. Disabled TLS supplies no active
   references or material, while auth references remain independent.
   Webhook credentials use its actual request-template Secret references, rather
   than a provider auth field; resolved values enter its schema-hidden transport slot.
2. Before any new dependency registration, Grant authorization or material read,
   the pipeline validates field/source identity and the exact dependency owner.
   Invalid identity creates no new dependency or lookup. A valid missing or denied
   reference registers its recovery dependency before authorization and lookup.
   Namespace defaults and cross-namespace permission come from the owning field;
   status-only shared entry points do not register dependencies.
3. Authorized Secret material is checked for the provider's accepted Secret type,
   required keys and decode rules, then embedded in the corresponding transport
   slot. `#[schemars(skip)]` excludes that slot from the CRD, serde carries it in
   conf_sync, and the redaction adapter masks Admin output. See
   [00-schema-transport-contract.md](00-schema-transport-contract.md).
4. Parse aggregates reference errors and maintains `ResolvedRefs`, preserving
   missing/denied recovery and malformed-material diagnostics. Auth failure does
   not suppress independent TLS processing. Enabled TLS publishes its complete
   resolved block only when its resolution has no error; material cleanup keeps
   disabled or failed selections from retaining the previous generation's bytes.
   CredentialSource retains its separate manager/publication fences and excludes
   disabled TLS inputs from the active selection.

`update_status` preserves the `ResolvedRefs` condition maintained by parse. It
updates Accepted/unknown-field and CredentialSource projections; it does not
re-read the reference pipeline or create dependency registrations.

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

The CredentialSource manager snapshot retains an Arc of the actual LinkSys.
The handler clones its scrubbed owner before moving authorized TLS material into
that private snapshot; the returned conf_sync resource stays scrubbed. Snapshot
clones share the Arc and expose neither Serialize nor Debug. Disabled TLS policy
remains present but inert, with no material or TLS dependency/input revisions.
The async manager captures the owner and global defaults through PreparedLinkSysTls
before DNS preparation, and OAuth2 builds with the canonical proved HTTP factory.
Endpoint, scopes and egress derive from that same owner. Existing live source,
Secret, Grant, issuer and leadership checks still control each acquisition and
publication; snapshot capture grants no freshness or authorization bypass.

OAuth2 provider preparation applies Common egress filtering to literal hosts of
the same parsed token endpoint retained for acquisition (`ARCH-104`), because the
HTTP connector bypasses DNS for IP literals. The source's `blockPrivate` policy
and shared integration-testing loopback exception remain canonical. Domain hosts
continue through the configured `EgressResolver` at connection time; preparation
does not pre-resolve them. Blocked literals return the existing fixed permanent
provider error before issuer IO. Isolated provider-construction tests cover
literal policy and URL retention; they do not exercise issuer or TLS handshakes.

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

### Local credential result ownership (ARCH-105)

`CredentialSourceRegistry` stores at most one manager result per logical key,
privately qualified by producing source UID, generation and the canonical frozen
input revision. Insertion compares against the current desired source under the
same write lock; reads validate under one read lock. A deleted source cannot gain
a late result, and same-name replacement or changed inputs cannot inherit the
previous observation. Identical re-registration preserves unchanged-result requeue
suppression. Qualification is Controller-only metadata, not a schema/conf_sync
field or another credential-material owner.

The manager validates its SourceState identity/revision before constructing that
observation, including cleanup completions which separately obtain current desired
state. An accepted current failed acquisition can still report retained last-good
publication; the qualification binds the observation rather than token issuance.
Same-qualified expiry remains visible. Old-state expiry after a source/input change
is not relabeled current, while memory clearing, historical Secret cleanup and
consumer notifications continue. A replacement may have no lifecycle result until
a qualifying current evaluation completes; no bounded convergence is promised.
The queue, retry policy and external publication/authority fences are unchanged.
See the [ownership regression guard](../../04-review/link-sys/regression-guards/credential-source-single-active-memory.md#local-manager-result-ownership-arch-105).

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

The Webhook provider calls `ManagedOutboundClient::from_webhook` with its actual
LinkSys binding and captured inherited TLS snapshot. The shared engine uses the
proved active TLS field to select the Service target's scheme and construct its
HTTP client. Disabled TLS remains in the owner but is inert for business calls
and active health checks; construction neither clones nor removes the block.
URL targets retain their explicit `http` or `https` scheme.

`edgion-common/src/link_sys_core/tls.rs` converts canonical effective TLS resolution into provider configuration. Actual-owner provider paths supply their proved field selection and captured inherited snapshot; the shared conversion cores retain the provider capability checks:
- `ca_pem`: derived from the resolved `validation.caCertificateRefs` Secret(s) carrying a `ca.crt` key. The old inline `TlsCerts.caCert` field has been removed. A custom CA replaces the System roots entirely.
- `client_cert_pem` / `client_key_pem`: from the Secret referenced by `clientCertificateRef`; the private key is `Zeroizing<Vec<u8>>`.
- `verify` is enforced per provider: Redis and Etcd always verify (`verify: false` is rejected at startup — the SDK has no skip-verify API); ES, Webhook, and other connectors honor `verify: false` as an operator decision (a WARN is emitted at config-load time). Trust anchors come from `validation.caCertificateRefs` (for LinkSys, only `kind: Secret` is supported in this release) or `validation.wellKnownCACertificates: System` (the process System CA bundle from `edgion_common::system_trust`, shared with Gateway backend TLS).

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


## Managed credential output correspondence

A LinkSys-backed CredentialInjector derives its generated Secret from the actual
consumer field: the Secret uses the EdgionPlugins namespace/name. Desired consumer
state retains the frozen field and no independent target strings. Frozen and
borrowed views share the same finite projection. Consumer/source correspondence
includes the captured LinkSys identity, source UID/generation and actual target;
it complements live owner, input, Grant and publication revision checks.

Injector and recovery qualified-store reads require explicit matching Secret
name/namespace before bearer parsing. This consumer does not acquire TLS's
bare-default fallback. Managed markers, expiry and single-active memory remain
independent. Pair validation also gates fan-out, completion acceptance, publication
desirability, published counts and memory clearing. Historical cleanup expectation
recovery remains independent of current token adoptability or live owner existence.
Managed Secret IO derives API coordinates from the private checked identity.
Initial GET, 409 winner, POST and JSONPatch results require explicit matching
name/namespace before markers or concurrency metadata are reused. POST creation,
field-manager-qualified 409 adoption, all JSONPatch CAS tests, and fenced immutable
recreation remain unchanged. Historical cleanup reconstructs only an expectation
from observed metadata and exact ownership; no live old owner is needed. Cleanup
rechecks identity after GET, then its original authorization closure, and deletes
with both Secret UID and resourceVersion preconditions. API namespace is strict: a
default target requires returned namespace `default`, never omitted metadata.


## Non-TLS auth material correspondence

Cold Redis/Etcd Basic, Elasticsearch Basic/API-key/Bearer, Kafka SASL and OTLP
Bearer consumers validate carried Secret metadata against their actual prepared
LinkSys auth field before parsing. The finite resolved_auth_secret method returns
an actual-owner borrow and uses the shared strict metadata comparator; it does not
copy material or lend TLS/bootstrap/template authority to auth. Common readiness
and both OTLP direct fingerprint/transport parsers use that same check. Preparation
runs on configuration/client construction paths, never per request.

No auth stays NotRequired, and absent carried auth stays Pending. Present invalid
reference or mismatched metadata produces the fixed ReferenceIdentity reason;
matching malformed payload still produces ParseFailed. OTLP retains its existing
unresolved-versus-invalid error categories and sensitive authorization headers.
Default namespace remains qualified-only for non-TLS auth: omitted owner/ref scope
means default, but material must explicitly match that namespace. FS/etcd bare
Secret lookup is not extended. No Secret UID or new Grant policy is added.

Ordinary same-type unbuildable updates post NotReady and retain the current client;
initially unbuildable configurations create none. A newer force_gen is handled
before NotReady and may retire the old client. Forced replacement, deletion,
type changes, retry/handover and captured transport lifetimes remain unchanged.
TLS, OAuth2 bootstrap, global defaults, egress and request behavior retain their
separate owning proofs and policies.
