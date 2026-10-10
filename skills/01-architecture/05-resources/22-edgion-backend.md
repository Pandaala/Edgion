---
name: resource-edgion-backend
description: EdgionBackend AI provider configuration, Controller credential resolution, route dependencies, conf_sync transport, and Gateway snapshots.
---

# EdgionBackend Architecture

`EdgionBackend` is a namespaced AI provider backend referenced by `HTTPRoute.backendRefs`.
Its required `spec.ai` configures the provider, endpoint, credential pool, model catalog, pricing,
and provider defaults. The `AiProxy` plugin activates AI protocol handling for a route rule.

## Controller processing

The Controller validates the AI configuration, resolves every credential Secret through
`AiApiKeyResolver`, and publishes the resource through conf_sync. Credential resolution is
partial-good: Controller preserves all declared credential slots for conf_sync; Gateway filters unresolved credentials from the usable pool, while zero usable credentials deactivates the
Gateway runtime entry. Apply and delete events requeue referencing HTTPRoutes and
EdgionBackendTrafficPolicies through the shared typed backend dependency index.

Resolved Secret objects remain serialized for the trusted Controller-to-Gateway channel, carry
`#[schemars(skip)]`, and use the redaction adapter so Admin API output never exposes secret values.

## Gateway runtime

`EdgionBackendStore` is the single ArcSwap-backed runtime store. It compiles each active resource
into one immutable `RuntimeAiBackend` containing:

- a normalized provider endpoint;
- resolved credentials and quota configuration;
- canonical, aliased, wildcard, and public model indexes;
- parsed fixed-point pricing;
- provider defaults and stable backend identity.

A request captures one `AiBackendSnapshot` generation for candidate selection and retries.
Equivalent updates reuse the existing runtime Arc and credential limiter state; invalid updates,
deletes, and credential loss atomically withdraw the entry.

## Route behavior

An EdgionBackend reference is meaningful only on an HTTPRoute rule activated by `AiProxy`.
The plugin selects among the rule's positive-weight AI candidates, resolves the requested model,
chooses a credential, and prepares the provider request. A raw EdgionBackend reference that reaches
ordinary forwarding fails closed; it is not treated as a Kubernetes Service or a generic external
endpoint.

BackendTLS and EdgionBackendTrafficPolicy attach to the direct EdgionBackend identity. AI provider
selection remains owned by AiProxy, so Service load balancing, health checks, and upstream authority
rewrites are not applied to the provider catalog.

## Credential-pool Redis source identity

After canonical spec validation, the cold compiler borrows the actual
credentialPool.redisRef through `refs::backend_fields`. An authored Redis field
requires a valid actual EdgionBackend name/namespace before UID derivation,
credential parsing or runtime reuse; neither a UID nor a valid-looking raw key
substitutes for that source. No Redis field adds no new source gate. Schema
validation still owns ref/limits pairing and the existing ASCII alphanumeric/-/_
target grammar. The runtime keeps the same key and live typed Redis lookup.

Equivalent backend/pool Arc reuse, counter identity, partial-good credentials and
captured snapshot lifetimes remain unchanged. This is a Redis-field boundary;
it adds no Grant and does not claim to validate the separate resolved Secret
material channel.

## Controller credential Secret field ownership

Each credential Secret resolution borrows its actual Backend and ordinal through
`refs::backend_fields`. Identity validation precedes dependency registration,
authorization and Secret acquisition; UID and raw workqueue keys cannot supply
missing source identity. Native omitted namespace still defaults to `default`,
while explicit empty source/target namespace is invalid. The custom Secret field
accepts omitted/empty/`core` group and omitted/`Secret` kind, with existing name rules.

The handler clears prior material and dependencies before rebuilding valid slots.
Valid missing or denied references retain recovery edges before authorization;
invalid source/target slots have neither new edges nor reads. Shared bound Secret
acquisition preserves ordinary material sanitization, AiApiKeyResolver validation,
partial-good credentials and structural status precedence. Invalid reference
outcomes use the bounded `InvalidReference` class, independently of Secret decoding
errors. This class is Controller-only status input; Secret transport/redaction is
unchanged. Other generic credential consumers remain separate review boundaries.


## Gateway credential material correspondence

The cold compiler enumerates actual credential ordinals through
BackendCredentialPoolField::secret_reference. Its borrowed resolved_secret
accessor validates the actual source/reference and returns only that slot's
carried Secret after exact name/namespace comparison. The shared private metadata
comparator also serves managed credential outputs without changing their policy.
Identity is checked before Secret type and API-key parsing and before runtime/pool
reuse. An invalid slot is omitted; a usable sibling keeps the backend active,
while zero usable credentials retains the fixed no-usable-credentials rejection.

Backend acquisition remains qualified-only. An omitted owner namespace means
`default`, but carried Secret metadata must explicitly match `default`; a raw
namespace-less Secret is not a TLS-style fallback. FS/etcd bare Secret storage and
its existing unresolved Backend outcome are unchanged. Custom core aliases remain
confined to the actual Secret reference profile. No Secret UID is required.

Schema validation, Redis-owner validation, runtime UID/key and endpoint checks
retain their existing priority before credential filtering. A malformed source
without Redis still makes its actual Secret slots unusable; no new top-level
schema owner requirement is introduced. Same-UID material mismatches cannot reuse
an old active pool. Equivalent valid inputs reuse their Arc/limiter as before,
and requests holding an older snapshot retain its immutable material. Controller
acquisition and Grant/status behavior were already bound and are not changed by
this additional cold consumer check.
