---
name: http-plugin-catalog
description: Full EdgionPlugins CRD schema and HTTP plugin catalog.
---

# EdgionPlugins — HTTP Plugins

> API: `edgion.io/v1` | Scope: Namespaced

The EdgionPlugins resource defines a set of HTTP-layer plugin configurations referenced by routes via ExtensionRef.

## Basic Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-plugins
  namespace: default
spec:
  waf:
    rules: |
      SecRule REQUEST_URI "@contains /blocked" "id:1,phase:1,deny,status:403"
    mode: on
  requestPlugins:
    - type: RateLimit           # Plugin type
      config:                   # Plugin configuration (varies by type)
        rate: 100
        burst: 200
        key: "$remote_ip"
    - type: Cors
      config:
        allowOrigins: ["*"]
```

## Plugin Catalog

### Authentication

| Type | Description | Secret Dependency |
|------|-------------|-------------------|
| `BasicAuth` | HTTP Basic authentication | ✅ username/password |
| `JwtAuth` | JWT token verification (RSA/EC/HMAC/EdDSA) | ✅ public/secret keys |
| `JweDecrypt` | JWE encrypted token decryption | ✅ encryption key |
| `HmacAuth` | HMAC signature verification; `validateRequestBody` verifies the buffered request `Digest`. Its flat standalone config composes the canonical match-only `HmacMatchConfig`. | ✅ access_key + secret_key |
| `KeyAuth` | API key authentication | ✅ API keys |
| `HeaderCertAuth` | Request-header / connection certificate authentication | — |
| `LdapAuth` | LDAP authentication | — |
| `ForwardAuth` | Forward to an external authentication service; `forwardBody` includes the buffered original request body | — |
| `OpenidConnect` | OpenID Connect / OAuth2 | ✅ client_secret + session_secret |

### Security

`spec.waf` is the native logical WAF. It is deliberately not an
`EdgionPlugin` discriminator and does not appear in any stage array. One
compiled generation internally owns request-header, request-body,
response-header, response-safe phase-4, and logging-phase execution. Its
schema is `{ rules: String, mode: on | detectionOnly }`; source is inline,
strictly compiled, admin-redacted, and cannot use filesystem/network includes
or external phrase assets. Invalid updates retain the last activatable
generation. See
`docs/en/user-guide/http-route/filters/edgion-plugins/waf.md`.

| Type | Description |
|------|-------------|
| `Cors` | CORS cross-origin resource sharing |
| `Csrf` | CSRF protection |
| `IpRestriction` | IP allow/deny list (CIDR supported). Inline `allow`/`deny` groups are **partial-good**: a bad CIDR entry is skipped (not fatal); the plugin loads with the valid entries. Bad entries surface as a `PartiallyInvalid` status condition (reason `RuleInvalid`); `Accepted` stays `True`. A skipped inline deny entry also increments `edgion_ip_deny_entries_skipped_total{scope="http"}`. |
| `RequestRestriction` | Request restrictions (header/body match rules). Geo matching is not part of this plugin — combine it with `GeoIpLocation` (below) as a separate, ordered plugin-list entry instead. |
| `RequestBodyBuffer` | Explicitly populate the shared request-body cache for later-stage consumers |
| `RequestAccessPolicy` | L7 named-profile access policy with independent `DirectPeerIp`/`RemoteIp` branches, IP-first and exact-host/path-first rules, bounded `anyOf`/`allOf` conditions, Secret/HMAC checks, and fail-closed Selector/IpList hot references. Its `hmacAuth` condition uses the strict match-only `HmacMatchConfig`; ordinary auth failures are non-matches, while non-forwardable body failures terminate evaluation. Geo matching is not part of this plugin — see `GeoIpLocation` below. |
| `GeoIpLocation` | Country/continent/city/subdivision (and CIDR) matching backed by the gateway's process-wide GeoIP provider, shared verbatim between this HTTP registration and the `EdgionStreamPlugins` L4 (Stage-1) registration of the same name. Parse-only (`config: {}`) just resolves geo into the access log; rule mode adds `defaultAction`/`failOpen`/`denyRuleGroups`/`allowRuleGroups`. Combine with `IpRestriction`/`RequestRestriction` by chaining plugin-list entries, not by embedding. See `docs/en/ops-guide/gateway/geoip.md`. |

### Traffic Control

| Type | Description |
|------|-------------|
| `RateLimit` | Local rate limiting (CMS algorithm) |
| `RateLimitRedis` | Distributed rate limiting (Redis backend) |
| `BandwidthLimit` | Bandwidth limiting |
| `FaultInjection` | Probabilistic delay and abort for controlled fault testing |
| `Canary` | Route all traffic to one named backend, switchable by Selector ConfigData |

### Request/Response Modification

| Type | Description |
|------|-------------|
| `RequestHeaderModifier` | Modify request headers (Gateway API standard) |
| `ResponseHeaderModifier` | Modify response headers (Gateway API standard) |
| `RequestRedirect` | HTTP redirect (Gateway API standard) |
| `UrlRewrite` | URL rewrite (Gateway API standard) |
| `RequestMirror` | Request mirroring (Gateway API standard) |
| `ProxyRewrite` | Proxy rewrite (advanced path/header rewriting and JSON request-body transforms) |
| `ResponseRewrite` | Response rewrite (status code/headers/body) |
| `RealIp` | Real IP extraction |
| `CtxSet` | Set context variables |
| `CredentialInjector` | Inject an upstream bearer or custom header from a Secret or a `credentialSource` LinkSys (`credentialRef.group: edgion.io`, `kind: LinkSys`). Default publication writes a generated Secret; `publication.persist: false` keeps generations in Controller memory (no generated Secret; see [18-link-sys.md](18-link-sys.md)). Header names and prefixes are static and never evaluate expressions. One EdgionPlugins resource may contain at most one enabled LinkSys-backed injector; static Secret-backed entries do not count toward that limit. |
| `WebhookKeyGet` | Resolve webhook vars into request context (once per request). Addressable in EdgionGatewayConfig `spec.pluginPolicy` allow/deny lists. |
| `ExtProc` | Envoy-compatible external processing. One `requestPlugins` entry owns one request-scoped Process stream across request headers/body and final response headers/body. Request bodies support `BUFFERED`/`STREAMED`; response bodies support bounded transport-window `STREAMED` only (no full-response aggregation and no semantic SSE-window claim). |
| `Guardrail` | One bounded HTTP Pass/Replace/Reject decision per canonical semantic AI SSE window. The request-stage owner installs an internal semantic response processor after AI translation/usage and before ordinary writable processors. Pass is byte-identical; policy Reject is never failed open. See `docs/en/user-guide/http-route/filters/edgion-plugins/guardrail.md`. |
| `Wasm` | Executes a proxy-wasm ABI v0.2 guest module (WebAssembly, run via wasmtime) in-process: header mutation, local responses, HTTP callouts (`calloutAllowlist`-gated; request phase with Pause/resume, and from `on_tick` with root-context callbacks when the module's plugins agree on one allowlist), buffered `proxy_on_request_body` when the request entry declares `body:`, read-only response-body observation, shared data (per-module KV with CAS), and `on_tick` timers. No body rewrite or gRPC callouts. Source forms: `file` / `inline` / `url` / `oci` (gateway-side pull from an OCI registry, with a bounded content-addressed process-memory cache). See `docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`. |
| `RequestId` | Incremental correlation IDs; W3C context is core gateway behavior |

### Routing/Backend

| Type | Description |
|------|-------------|
| `DirectEndpoint` | Directly specify a backend endpoint |
| `DynamicInternalUpstream` | Dynamic internal upstream |
| `DynamicExternalUpstream` | Dynamic external upstream |
| `AllEndpointStatus` | All-endpoint status query |
| `RegionRoute` | Multi-region traffic routing. The complete routing logic and safe default `regions` table live inline in EdgionPlugins; optional `overrideRef` targets a `RegionRouteOverride` EdgionConfigData for hot-swappable failover state. The Controller effective API resolves permitted/enabled overlays, preserves the reference namespace, and exposes the entry-level `dye` suite (request/response header rules keyed on outcome via `on`) plus a stable request-plugin entry index. Center service-usage views derive consumers from HTTPRoute/GRPCRoute ExtensionRefs targeting the containing EdgionPlugins resource, using `resolvedRules` when delegation is present so inherited filters reach the actual Service backends. See `05-resources/15-edgion-config-data.md`. |

> Note: `EdgionPlugin::ExtensionRef` (the nested form) has been removed. Multi-plugin reuse is achieved via an aggregate EdgionPlugins resource or by listing multiple ExtensionRef entries side-by-side in the HTTPRoute filter list. See `docs/en/migrations/<version>-extension-ref.md`; the paired Chinese page is under `docs/zh-CN/migrations/`.

### Debugging/Testing

| Type | Description |
|------|-------------|
| `Mock` | Mock response (testing/prototyping) |
| `DebugAccessLogToHeader` | Debug: inject the access log into a response header |
| `Dsl` | DSL script plugin (executed in a sandbox VM; see the DSL engine section in architecture `01-architecture/02-gateway/05-plugin-system.md`) |

## Plugin Execution Stages

Ordinary HTTP plugins execute across Pingora's 4 stages:

| Stage | Trigger Time | Typical Plugins |
|-------|--------------|-----------------|
| `spec.requestPlugins` (`RequestFilter`) | After the request is received and before backend selection | Auth, RateLimit, CORS, routing plugins, `RequestBodyBuffer`, `ExtProc`, `Guardrail`, `Wasm` |
| `spec.upstreamResponseFilterPlugins` (`UpstreamResponseFilter`) | After upstream response headers are received, synchronous | ResponseHeaderModifier, DSL, `Wasm` |
| `spec.upstreamResponseBodyFilterPlugins` (`UpstreamResponseBodyFilter`) | For each upstream response body chunk | ResponseRewrite, `Wasm` read-only observation |
| `spec.upstreamResponsePlugins` (`UpstreamResponse`) | After upstream response headers arrive, asynchronous | No directly configurable plugin currently; ExtProc installs an internal companion from its request entry |

Each entry uses the flattened wire shape `enable + type + config`. A plugin may
only be placed in stages admitted by the exhaustive Rust
`validate::applicable_stages` table; the same resource may contain entries in
multiple stage arrays. There is no generic `spec.plugins` field.

`spec.waf` is the sole exception to stage-array placement: it is one logical
top-level configuration whose internal companions run at every supported WAF
phase. At most one logical WAF may own a request; composing multiple
WAF-bearing EdgionPlugins references fails closed.

## Shared condition expressions

Plugin entry `conditions.skip` / `conditions.run`, `body.conditions`, dye rule
conditions, RequestRestriction, and RequestAccessPolicy supplemental
conditions all use the same bounded `ConditionSet` wire shape:

```yaml
conditions:
  anyOf:
    - type: keyExist
      key:
        type: header
        name: X-Internal-Request
    - name: production-write
      allOf:
        - type: keyMatch
          key:
            type: header
            name: X-Env
          values: [production]
        - type: keyMatch
          key:
            type: method
          values: [POST]
```

The top level must contain exactly one non-empty `allOf` or `anyOf`.
Top-level `allOf` accepts direct `Condition` leaves only. Top-level `anyOf`
accepts direct leaves or one-level `{ name?, allOf: [...] }` groups. Nested
`allOf` contains direct leaves only; `allOf -> anyOf`, `anyOf -> anyOf`, deeper
recursion, mixed operators, empty arrays, and blank group names
are invalid. `conditions.skip` is evaluated before `conditions.run`, but each
field's Boolean behavior is selected by its contained set rather than implied
by the field name.

### `enable: false` means bypass, not reject

An entry-level `enable: false` is **semantically inert**: the entry is not
built, not run, and cannot invalidate the containing resource through its
dormant inner configuration. Both the Controller schema layer
(`EdgionPlugins::validate_plugins`) and the Gateway compilation
(`compile_plugin_conditions` / `add_from_*`) skip disabled entries for config,
alias, stage, dependency (Secret / LinkSys / EdgionConfigData ref), and
`conditions` regex/`timeRange` validation. A parked entry that carries a
malformed condition or invalid config therefore keeps the resource
`Accepted=True` — it is a no-op, not a validation error. This matches the
EdgionStreamPlugins rule (`preparse_skips_disabled_plugins`); HTTP and stream
follow one rule.

This is distinct from an **enabled invalid** entry (`enable: true` + bad
config): that seals the whole EdgionPlugins resource fail-closed
(`seal_fail_closed`), so every request referencing it gets HTTP 500. In short:
disabling an auth/authorization plugin intentionally lets the route continue
without it (bypass); an invalid *active* plugin must never fail open. The
distinction is:

| Entry state | Built / run | Inner config + conditions validated | Effect on the resource |
|-------------|-------------|-------------------------------------|------------------------|
| enabled and valid | yes | yes | none |
| disabled (`enable: false`) | no | no | none (stays `Accepted=True`) |
| enabled and invalid | no | yes | resource fails closed (HTTP 500 per request) |

## Secret Dependencies

The following plugin types read Secret data from `ControllerRuntime.secret_store` during the Controller-side `parse()` stage:

| Plugin | Secret Fields Read |
|--------|--------------------|
| `BasicAuth` | username, password |
| `JwtAuth` | RSA/EC/HMAC/EdDSA public keys, key metadata |
| `JweDecrypt` | JWE encryption key |
| `HmacAuth` | access_key, secret_key |
| `KeyAuth` | API keys |
| `OpenidConnect` | client_secret, session_secret |
| `HeaderCertAuth` | CA certificates |
| `Wasm` | `source.fetch`: artifact-repo Authorization header value, fetch TLS CA / client certs |
| `Wasm` | `source.oci.pullSecretRef`: `kubernetes.io/dockerconfigjson` registry credentials, resolved into `resolved_pull_secret` (redacted in admin output); unresolved ⇒ plugin load failure |
| `Guardrail` | `${secretRef:namespace/name:key}` values used by restricted request path/custom-header templates, plus outbound TLS CA/client credentials |
| `CredentialInjector` | Static Secret: `bearerToken`. `credentialSource` LinkSys: active token from a generated Secret (`persist: true`) or `ControllerRuntime.credential_memory` (`persist: false`); never from a LinkSys conf_sync field |

Secret changes automatically trigger reprocessing of EdgionPlugins via SecretRefManager.

## GatewayClass plugin execution policy

`EdgionGatewayConfig.spec.pluginPolicy` governs `http/<Type>` entries in all four HTTP stages.
The actual GatewayClass binding selects the policy, so shared plugin resources
can be allowed in one class and restricted in another. The Gateway publishes
compatible views dynamically; the Controller neither stamps entries nor emits
a global restriction condition on the shared plugin resource.

See [spec.pluginPolicy](../02-config/02-edgion-gateway-config.md#specpluginpolicy)
for allow/deny semantics, lifecycle boundaries and unavailable-policy behavior.

## Unknown config keys (`UnknownConfigFields` condition)

Because a plugin's `config` block is `x-kubernetes-preserve-unknown-fields` in the CRD, the
Kubernetes API server does not prune a mistyped key (e.g. `denys:` instead of `deny:`), and
serde silently drops it during deserialization — the resource keeps running with that key
simply ignored, which for a security plugin is a fail-open, not a no-op. The Controller
detects this at ingest and reports it:

```yaml
status:
  conditions:
    - type: UnknownConfigFields
      status: "True"
      reason: UnknownFieldsDetected
      message: "requestPlugins[0].config.denys"
```

- `Accepted` is **not** touched — it keeps meaning "resource valid and synced". The config
  still syncs to the Gateway exactly as it would without this check; the mistyped key is
  still ignored, unchanged from today's behavior.
- The condition does **not** feed `edgion_controller_resource_abnormal` — an operator typo
  is not resource ill-health.
- The Controller also emits a WARN log line with the full path list.
- To clear the condition, fix the key and re-apply; the next apply removes the condition
  (upsert-on-hit, remove-on-fix — it is never left wedged at a stale `True`).

Full operator guidance — including the fleet-wide `GET /api/v1/diagnostics/unknown-fields`
endpoint and the `POST /api/v1/validate/{kind}` endpoint for building your own blocking
gate — is in `docs/en/ops-guide/plugin-config-unknown-fields.md` and
[Admin API — diagnostics/validate](../../01-architecture/01-controller/02-admin-api.md).

## RequestId configuration and phases

`defaultId` is an optional patch with `enabled`, `forwardUpstream`, and `echoResponse`; absent fields leave previous policy unchanged. `ids` adds destination headers with optional shared KeyGet `source`, `generateWhenMissing` (true), `generator`, `forwardUpstream` (true), and `echoResponse` (true). Generators are uuidV4 (default), uuidV7, randomHex (`bytes`: 8-64, default 16), nanoId (`length`: 8-128, default 21), and md5 over fresh random bytes. `prefix` applies only to newly generated values.

Only Header and TraceId sources work at early Global/Gateway attachments; route entries may also use permitted Ctx keys. Early conditions accept KeyExist/KeyMatch over Header, Method or TraceId; body/dye wrappers are rejected. BackendRef scope is invalid. Schema validation is shared with the Gateway and attachment dependencies requeue after plugin updates. Rules accumulate across tiers, while disabling the implicit default does not remove explicitly configured custom IDs. Core W3C propagation remains enabled independently.

See [complete RequestId examples and bounds](../../../docs/en/user-guide/http-route/filters/edgion-plugins/request-id.md).
## Local rate-limit overflow composition

RateLimit.config.onLimitExceeded is Reject (default) or Continue. Continue records
local excess without rejecting; a later request-stage RateLimitRedis can use the
shared Condition `rateLimitExceeded: { pluginAlias }` (tagged YAML form:
`type: rateLimitExceeded`, `pluginAlias: local-budget`) in conditions.run.
The alias must name an active earlier RateLimit with Continue in the same
requestPlugins array. Other stages/consumers, skip/body/dye placement, and invalid
references are rejected. Controller validation and Gateway sealing share this
contract. Disabled consumers remain inert. Missing runtime producer evaluation
fails closed before Boolean short-circuiting; WithinLimit skips Redis and Exceeded
runs it. Extra valid conditions can intentionally bypass Redis.

Outcome slots are Gateway-owned and invocation-local, not serialized operator
fields or writable ctx variables. Existing local scope/CMS accounting and Redis
failure policy remain unchanged. See the user-facing overflow recipe in
docs/en/user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md.
