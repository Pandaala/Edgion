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
partial-good: unresolved credentials are omitted, while zero usable credentials deactivates the
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
