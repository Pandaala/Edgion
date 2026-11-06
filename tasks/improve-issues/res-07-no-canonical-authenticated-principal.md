# res-07: No canonical authenticated principal — every auth plugin projects identity its own way, consumers key on request headers

- **Category**: best-practice
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `edgion-resources/src/common/key_accessor.rs:60-120` (`KeyGet`: RemoteIp/Header/Cookie/Query/Ctx/... — no principal source)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/basic_auth.rs:85-87`, `key_auth.rs:74-124`, `jwt_auth.rs:132-156`, `hmac_auth.rs:29-36`, `ldap_auth.rs:71-107`, `header_cert_auth.rs:35-110`, `forward_auth.rs:32-46` (per-plugin identity/anonymous knobs)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs:211-236` (`AiTokenQuotaConfig.key: Vec<KeyGet>`)
  - `edgion-gateway/src/plugins/http/basic_auth/plugin.rs:165-177`, `key_auth/plugin.rs:39-186`, `jwt_auth/plugin.rs:96` (identity written as `X-Consumer-Username` header or `jwt_claims` ctx)

## Summary

Edgion ported Kong-style auth plugin options (`anonymous`, `hideCredentials`, `X-Consumer-Username`) but not
Kong's central idea: a *consumer* (principal) object that every auth plugin resolves to and every later
plugin can rely on. Each auth plugin has its own identity projection (different header names, different
anonymous models: `anonymous: Option<String>` vs `allowAnonymous: bool` vs `anonymousHeader`), and downstream
policy (rate limits, AI token quotas, ACL conditions, access log) must identify the caller by reading a
request header or a plugin-specific ctx key through `KeyGet`. The trusted principal is therefore
indistinguishable from client-supplied input whenever the auth plugin did not run.

## Current behavior (evidence)

- Identity projection differs per plugin: BasicAuth/KeyAuth/HMAC write `X-Consumer-Username` (request
  header, overwritten only on the paths that set it); LDAP writes `credentialIdentifierHeader`
  (`X-Credential-Identifier`) and `anonymousHeader` (`X-Anonymous-Consumer`); HeaderCertAuth writes
  `upstreamHeaders.consumerHeader` with `consumerBy` and `skipConsumerLookup` ("consumer mapping" with no
  consumer registry); JwtAuth stores `jwt_claims` in ctx and projects claims to headers; ForwardAuth copies
  arbitrary `upstreamHeaders`.
- Anonymous semantics differ: `anonymous: Option<String>` (Basic/Key/JWT/HMAC/LDAP) vs
  `allowAnonymous: bool` (HeaderCertAuth).
- `KeyGet` has no "authenticated principal" variant. Rate limiting and `AiTokenQuotaConfig.key` identify the
  caller with `KeyGet::Header{"X-Consumer-Username"}`/`Ctx{..}` — the same header a client can send. If the
  auth entry is skipped (entry `conditions`, a different route, a plugin ordered after the limiter), the
  client-supplied value is used as identity.
- Credentials are configured per auth-plugin instance (`secretGroups`), so one caller holding a basic-auth
  password and an API key is two unrelated identities; there is no per-principal plugin override or group.

## Industry comparison

- Kong: `KongConsumer` (+ credentials, `KongConsumerGroup`); every auth plugin sets `kong.client.authenticate
  (consumer, credential)`; rate-limiting/ACL/quotas key on the consumer; plugins can be scoped per consumer
  or consumer group.
- APISIX: `consumer` / `consumer_group` objects; auth plugins set `ctx.consumer_name`; `limit-count` keys on
  `consumer_name`.
- Envoy/Envoy Gateway: authn filters write typed dynamic metadata / filter state (JWT payload in metadata,
  ext_authz dynamic metadata); RBAC and rate-limit descriptors read that metadata, never a client header.

## Impact

- Spoofable identity for quota/rate-limit/ACL configurations that key on the projected header.
- AI Gateway features (per-caller token budgets, model access) need exactly this principal; the current
  approach forces each deployment to hand-assemble it (the pending AI task recognizes the gap).
- Inconsistent knobs across seven auth plugins increase documentation and review load.

## Suggested direction

1. Introduce one gateway-owned principal slot in the request context (e.g. `{ source_plugin, id, groups,
   anonymous }`) written only by auth plugins through one API; strip any inbound header with the
   configured projection name before plugins run.
2. Add `KeyGet::Principal` (and `Principal.group`) and a `Condition` on principal/group; make rate limits, AI
   quotas and access-log fields use it.
3. Normalize auth plugin identity/anonymous config to one shared struct (projection header name, anonymous
   principal, hide credentials).
4. Optionally, later: a namespaced `EdgionConsumer`-like credential/group catalogue (could be an
   `EdgionConfigData` variant) so one principal can own several credential types.

## Notes

- `tasks/todo/ai-todo/trusted-identity-and-model-access.md` covers the AI-specific consumer of this (binding
  trusted identities to model access) and explicitly wants to reuse "canonical identity/key/condition
  machinery"; this finding identifies that no such canonical principal exists yet and proposes the general
  mechanism it would build on.
- Checked `skills/04-review/plugin-metadata/mandatory-design/keyget-is-local-io-free.md` (KeyGet must stay
  local and IO-free — a ctx-backed principal satisfies that) and the auth-plugin review folders; none rejects
  a canonical principal.

## Related findings

- gw-plugin-01, gw-plugin2-02 (client-forged identity headers reaching upstream / mirrors) — a canonical principal would give these a single trusted source.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged (direction with a real spoofing exposure in common limiter/quota configs).
- `basic_auth/plugin.rs:176-188` projects identity only by setting the `X-Consumer-Username` request header (anonymous projection also sets `X-Anonymous-Consumer`). `key_auth/plugin.rs:39,79,186` uses the same header.
- No principal type or `KeyGet` principal source exists (`grep Principal` in `edgion-gateway/src` hits only `ai/runtime/attempt_plan.rs`, which is unrelated). Downstream keying therefore reads a header that a client can also send when the auth entry did not run.
- Overlaps gw-plugin-01 and gw-plugin2-02 (other batches) for the header-forgery symptom. This finding is the structural fix.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `key_accessor.rs` is unchanged; `KeyGet` still has no principal source.
- BasicAuth now verifies inline instead of via `spawn_blocking`, but identity is still projected only as the `X-Consumer-Username`/`X-Anonymous-Consumer` request headers (`basic_auth/plugin.rs:165-177`). Line ref updated.
- `auth_header_projection.rs` adds a Host-destination rejection for JWT/OIDC claim projection; it does not introduce a canonical principal.
