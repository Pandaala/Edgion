# res4-04: `namespace/name` LinkSys references (Redis, Webhook, DSL, AI quota, EdgionBackend) cross namespaces with no ReferenceGrant, letting any tenant drive another tenant's Redis and credentialed webhooks

- **Category**: design-error
- **Severity**: P1 (security)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/refs/plugin_fields.rs:395-433` (`RateLimitRedis.redisRef`, `GraphqlRateLimit.redisRef`, `WebhookKeyGet.vars[].webhookRef`, `AiProxy.tokenQuota.redisRef`, `Dsl.linksys_refs[]` are only snapshot atoms)
  - `edgion-resources/src/resources/refs/backend_fields.rs:47-58` (`BackendCredentialPoolField::redis_reference` returns the raw string; "preserving its exact target string")
  - `edgion-resources/src/resources/edgion_stream_plugins/connection_rate_limit_config.rs:123-141` (`ConnectionRateLimit.redisRef`: lexical check only)
  - `edgion-resources/src/resources/edgion_backend.rs:732-743` (`valid_redis_ref`: lexical only)
  - `edgion-gateway/src/link_sys/runtime/store.rs:610-616` (`get_redis_client(name)`: global `namespace/name` lookup, no caller identity)
  - `edgion-resources/src/resources/link_sys/webhook.rs:87-93` (Webhook LinkSys carries `resolved_secrets`)
  - Contrast: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/credential_injector.rs:110-160` (CredentialInjector's LinkSys reference *does* require a ReferenceGrant)

## Summary

LinkSys objects are namespaced and hold connection credentials (Redis auth, webhook request templates with
resolved Secrets, etcd/ES/Kafka auth). Consumers reference them with a free-form `"namespace/name"`
string. For every string-typed consumer - RateLimitRedis, GraphqlRateLimit, WebhookKeyGet, AiProxy token
quota, DSL `linkSysRefs`, stream ConnectionRateLimit and `EdgionBackend.credentialPool.redisRef` - neither
the Controller nor the Gateway checks a ReferenceGrant or any other consent from the target namespace.
The Gateway resolves the string in a process-global table. The only LinkSys consumer that does go through
the grant store is CredentialInjector, which shows the project's own intended rule.

## Current behavior (evidence)

- Validation of these fields is lexical (`check_redis_ref`, `valid_redis_ref`, DSL/plugin schema checks).
  `plugin_fields.rs` records them as `Atom::LinkSys { namespace, name }` purely for stale-binding detection;
  no `grant_query()` is produced for them (compare `credential_injector.rs:146-155`, which calls
  `ref_grant_store.query_authorization_revision(reference.grant_query())` and fails with `RefNotPermitted`).
- `grep` over `edgion-controller/src` finds no use of `redis_ref`, `webhook_ref` or `linksys_refs` in any
  handler; `link_sys_ref_mgr.add_ref` is called only from the CredentialInjector path.
- At runtime `get_redis_client(&self.config.redis_ref)` (`rate_limit_redis/plugin.rs:384`,
  `connection_rate_limit/plugin.rs:168`, `ai/runtime/attempt_runtime.rs:689`) looks the key up in
  `RUNTIMES` with no notion of the calling resource's namespace.
- `WebhookServiceConfig.resolved_secrets` (`webhook.rs:87-93`) means a webhook LinkSys call carries the
  owning namespace's Secret material (e.g. an API key header) to its target.

## Industry comparison

- Gateway API: every cross-namespace object reference requires a ReferenceGrant in the target namespace.
- Kong/APISIX/Envoy Gateway: shared infrastructure connectors (Redis for rate limiting, ext-auth services)
  are either cluster-admin configured or same-namespace; Envoy Gateway `BackendTrafficPolicy` rate limit uses
  a single admin-configured Redis, never a tenant-selected one.

## Impact

A tenant who can create an `EdgionPlugins` (or `EdgionBackend`, `EdgionStreamPlugins`) in namespace A can:
- write arbitrary rate-limit counters into namespace B's Redis (`RateLimitRedis`, `GraphqlRateLimit`,
  `ConnectionRateLimit`), exhausting B's quotas or polluting its keyspace; with DSL `linkSysRefs` read/write
  keys through the DSL Redis builtins;
- make the Gateway call B's webhook with B's resolved credentials and inject the extracted values into A's
  request (`WebhookKeyGet`), a confused-deputy/credential-use path;
- point `EdgionBackend.credentialPool.redisRef` / `AiProxy.tokenQuota.redisRef` at B's Redis to read or
  corrupt B's AI quota accounting.
The documented "share data via EdgionConfigData with ReferenceGrant" model is bypassed for every
infrastructure connector except CredentialInjector.

## Suggested direction

1. Recommended: route every `namespace/name` LinkSys consumer through the same reference contract
   CredentialInjector uses (`ReferenceToken` -> `grant_query()` -> `ref_grant_store`), with
   `from: <consumer kind>/<ns>` -> `to: edgion.io/LinkSys`. Same-namespace references stay implicit.
   Register `link_sys_ref_mgr` / `cross_ns_ref_mgr` edges so Grant changes requeue consumers, and transport
   an authorization result (or blank the reference) so the Gateway fails closed if the Controller is
   bypassed.
2. Alternatively, make cross-namespace use explicit on the provider side (e.g. a LinkSys
   `allowedNamespaces`/visibility field like `EdgionConfigData.visibility`), defaulting to same-namespace.
3. Either way, the gateway-side `get_redis_client` should receive the consumer's namespace and refuse a
   foreign key that was not authorized.

## Notes

- Distinct from `res-01` (Gateway annotation attaches another namespace's whole EdgionPlugins). Here the
  EdgionPlugins/EdgionBackend is in the attacker's own namespace and directly names the victim's LinkSys.
- `tasks/pending/resource-reference-contract/02-field-manifest.md` lists these fields under "Fixed
  identities, strings, and selectors" with the instruction to *preserve* the existing explicit
  `namespace/name` grammar; it does not record a decision that cross-namespace use without a grant is
  intended, and CredentialInjector shows the opposite rule. The refactor task is the natural place to add
  the grant query.
- Checked `skills/01-architecture/05-resources/18-link-sys.md` ("cross-namespace permission come[s] from the
  owning field") and `skills/04-review/link-sys/` pages; none accept ungranted cross-namespace LinkSys use.

## Verification (independent)

- **Verdict**: CONFIRMED (P1)
- `edgion-gateway/src/link_sys/runtime/store.rs:610-616` `get_redis_client(name)` resolves a global `namespace/name` key from `RUNTIMES` with no caller namespace.
- `grep` over `edgion-controller/src` for `redis_ref|webhook_ref|linksys_refs` (non-test) finds no handler use; `link_sys_ref_mgr` is used only by `link_sys.rs`, `edgion_plugins.rs` and the CredentialInjector path, which alone performs a ReferenceGrant check.
- No page under `skills/04-review/link-sys/` or `skills/04-review/rate-limit/` accepts ungranted cross-namespace LinkSys use; `18-link-sys.md:129` says cross-namespace permission comes from the owning field, which these string fields never grant.
- Duplicate check vs `res-01`: different mechanism (res-01 = Gateway annotation attaching another namespace's EdgionPlugins; here the consumer in the attacker's namespace names the victim's LinkSys). Both kept.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1
- `refs/plugin_fields.rs`, `refs/backend_fields.rs`, `edgion_stream_plugins/`, `edgion_backend.rs`, `link_sys/webhook.rs` and `credential_injector.rs` are unchanged; still no ReferenceGrant on Redis/Webhook/DSL refs.
- `link_sys/runtime/store.rs` only adds a Redis connection-coverage metric; `get_redis_client(name)` (`store.rs:610`) is still a global lookup without caller identity. `link-sys` review page changes concern Redis config/health/egress, not cross-namespace authorization.
