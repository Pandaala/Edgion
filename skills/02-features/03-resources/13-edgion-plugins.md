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
  requestPlugins:
    - type: RateLimit           # Plugin type
      config:                   # Plugin configuration (varies by type)
        rate: 100
        burst: 200
        key: "$remote_addr"
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
| `HmacAuth` | HMAC signature verification; `validateRequestBody` verifies the buffered request `Digest` | ✅ access_key + secret_key |
| `KeyAuth` | API key authentication | ✅ API keys |
| `HeaderCertAuth` | Request-header / connection certificate authentication | — |
| `LdapAuth` | LDAP authentication | — |
| `ForwardAuth` | Forward to an external authentication service; `forwardBody` includes the buffered original request body | — |
| `OpenidConnect` | OpenID Connect / OAuth2 | ✅ client_secret + session_secret |

### Security

| Type | Description |
|------|-------------|
| `Cors` | CORS cross-origin resource sharing |
| `Csrf` | CSRF protection |
| `IpRestriction` | IP allow/deny list (CIDR supported). Inline `allow`/`deny` groups are **partial-good**: a bad CIDR entry is skipped (not fatal); the plugin loads with the valid entries. Bad entries surface as a `PartiallyInvalid` status condition (reason `RuleInvalid`); `Accepted` stays `True`. A skipped inline deny entry also increments `edgion_ip_deny_entries_skipped_total{scope="http"}`. |
| `RequestRestriction` | Request restrictions (header/body/geo rules). `RestrictionSource::ClientGeo`/`DirectGeo` add country/continent/city/subdivision matching, backed by the gateway's process-wide GeoIP provider — see `docs/en/ops-guide/gateway/geoip.md`. |
| `RequestBodyBuffer` | Explicitly populate the shared request-body cache for later-stage consumers |
| `GlobalAccessControl` | L7 allow-list with IP/CIDR and composite request match groups; shares `RestrictionSource` with `RequestRestriction`, so it also gets `ClientGeo`/`DirectGeo` — note its `IpSource` naming is inverted relative to `RequestInfo` (see the GeoIP guide). |

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
| `ExtProc` | Envoy-compatible external processing. One `requestPlugins` entry owns one request-scoped Process stream across request headers/body and final response headers/body. Request bodies support `BUFFERED`/`STREAMED`; response bodies support bounded transport-window `STREAMED` only (no full-response aggregation and no semantic SSE-window claim). |
| `Guardrail` | One bounded HTTP Pass/Replace/Reject decision per canonical semantic AI SSE window. The request-stage owner installs an internal semantic response processor after AI translation/usage and before ordinary writable processors. Pass is byte-identical; policy Reject is never failed open. See `docs/en/user-guide/http-route/filters/edgion-plugins/guardrail.md`. |
| `A2aGateway` | Publishes an operator-owned A2A v1 Agent Card on a backendless exact discovery route and observes a bounded JSON-RPC method on typed A2A routes. It does not fetch or rewrite backend-returned cards. See `docs/en/user-guide/http-route/filters/edgion-plugins/a2a-gateway.md`. |
| `Wasm` | Executes a proxy-wasm ABI v0.2 guest module (WebAssembly, run via wasmtime) in-process: header mutation, local responses, HTTP callouts (`calloutAllowlist`-gated; request phase with Pause/resume, and from `on_tick` with root-context callbacks when the module's plugins agree on one allowlist), buffered `proxy_on_request_body` when the request entry declares `body:`, read-only response-body observation, shared data (per-module KV with CAS), and `on_tick` timers. No body rewrite or gRPC callouts. Source forms: `file` / `inline` / `url` / `oci` (gateway-side pull from an OCI registry, with a content-addressed disk cache). See `docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`. |
| `TraceContext` | W3C trace-context propagation and normalization |

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

HTTP plugins execute across Pingora's 4 stages:

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

## `A2aGateway` configuration

`A2aGateway` is valid only in `spec.requestPlugins`.

| Field | Type | Default | Contract |
|-------|------|---------|----------|
| `card` | JSON object | required | Operator-owned A2A v1 Agent Card; compact JSON is capped at 1 MiB, the required core shape is validated, and at least one `JSONRPC`/`1.0` interface is required; optional/extension fields are preserved opaquely |
| `externalUrl` | HTTP(S) origin | routed origin | Trusted publication origin; user information, path, query, fragment, whitespace, and control characters are rejected |
| `failOpen` | Boolean | `false` | On an A2A version/content-type gate mismatch, forward unchanged instead of returning `400`; never changes body-storage failures |

The backendless local-card rule must be exact `GET /.well-known/agent-card.json`
and have no backend. An RPC rule must use only
positive-weight `edgion.io/EdgionBackend` references whose available runtime
variants are A2A. The route-level ExtensionRef points to the containing
EdgionPlugins resource in the normal Gateway API form.

Detailed required fields, snapshot pinning, and body failure semantics are
canonical in [`A2A Gateway Phase 1`](../../01-architecture/02-gateway/15-a2a-gateway.md).

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

Secret changes automatically trigger reprocessing of EdgionPlugins via SecretRefManager.

## Controller Plugin Policy (Allowlist / Denylist)

An operator can restrict which `http/<Type>` plugin types the Gateway is
allowed to instantiate via the Controller's optional
`validation.plugin_policy` config section (absent by default = allow all).
A restricted entry is stamped, not force-disabled, and is reported via a
`PluginPolicyRestricted` status condition. See
[validation.plugin_policy](../02-config/00-controller-config.md#validationplugin_policy)
for the config schema and `docs/en/ops-guide/plugin-policy.md` for the
operator guide.
