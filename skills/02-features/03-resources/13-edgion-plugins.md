---
name: http-plugin-catalog
description: Full EdgionPlugins CRD schema and catalog of 35 HTTP plugins.
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
  plugins:
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
| `HmacAuth` | HMAC signature verification | ✅ access_key + secret_key |
| `KeyAuth` | API key authentication | ✅ API keys |
| `HeaderCertAuth` | Request-header / connection certificate authentication | — |
| `LdapAuth` | LDAP authentication | — |
| `ForwardAuth` | Forward to an external authentication service | — |
| `OpenidConnect` | OpenID Connect / OAuth2 | ✅ client_secret + session_secret |

### Security

| Type | Description |
|------|-------------|
| `Cors` | CORS cross-origin resource sharing |
| `Csrf` | CSRF protection |
| `IpRestriction` | IP allow/deny list (CIDR supported). Inline `allow`/`deny` groups are **partial-good**: a bad CIDR entry is skipped (not fatal); the plugin loads with the valid entries. Bad entries surface as a `PartiallyInvalid` status condition (reason `RuleInvalid`); `Accepted` stays `True`. A skipped inline deny entry also increments `edgion_ip_deny_entries_skipped_total{scope="http"}`. |
| `RequestRestriction` | Request restrictions (header/body rules) |

### Traffic Control

| Type | Description |
|------|-------------|
| `RateLimit` | Local rate limiting (CMS algorithm) |
| `RateLimitRedis` | Distributed rate limiting (Redis backend) |
| `BandwidthLimit` | Bandwidth limiting |

### Request/Response Modification

| Type | Description |
|------|-------------|
| `RequestHeaderModifier` | Modify request headers (Gateway API standard) |
| `ResponseHeaderModifier` | Modify response headers (Gateway API standard) |
| `RequestRedirect` | HTTP redirect (Gateway API standard) |
| `UrlRewrite` | URL rewrite (Gateway API standard) |
| `RequestMirror` | Request mirroring (Gateway API standard) |
| `ProxyRewrite` | Proxy rewrite (advanced path/header rewriting) |
| `ResponseRewrite` | Response rewrite (status code/headers/body) |
| `RealIp` | Real IP extraction |
| `CtxSet` | Set context variables |
| `ExtProc` | External processing over a gRPC bidirectional stream (Envoy `ext_proc` compatible): request/response header mutation + immediate-response short-circuit. MVP is header-only (no body); peer of `ForwardAuth`. Runs in the `RequestFilter` and async `UpstreamResponse` stages. |
| `Wasm` | Executes a proxy-wasm ABI v0.2 guest module (WebAssembly, run via wasmtime) in-process: header mutation, local responses, HTTP callouts (`calloutAllowlist`-gated; request phase with Pause/resume, and from `on_tick` with root-context callbacks when the module's plugins agree on one allowlist), read-only response-body observation, shared data (per-module KV with CAS), and `on_tick` timers. No request-body hooks, no body rewrite, no gRPC callouts. Source forms: `file` / `inline` / `url` / `oci` (gateway-side pull from an OCI registry, with a content-addressed disk cache). Runs in the `RequestFilter`, sync `UpstreamResponseFilter`, and `UpstreamResponseBodyFilter` stages. See `docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`. |

### Routing/Backend

| Type | Description |
|------|-------------|
| `DirectEndpoint` | Directly specify a backend endpoint |
| `DynamicInternalUpstream` | Dynamic internal upstream |
| `DynamicExternalUpstream` | Dynamic external upstream |
| `AllEndpointStatus` | All-endpoint status query |
| `RegionRoute` | Multi-region traffic routing. The EdgionPlugins config only holds a `serviceRegionRouteRef`; all routing rules live in EdgionConfigData (ServiceRegionRoute → ClusterRegionRoute), resolved at plugin-build time. See `05-resources/15-edgion-config-data.md`. |

> Note: `EdgionPlugin::ExtensionRef` (the nested form) has been removed. Multi-plugin reuse is achieved via an aggregate EdgionPlugins resource or by listing multiple ExtensionRef entries side-by-side in the HTTPRoute filter list. See `docs/zh-CN/migrations/<version>-extension-ref.md` for details.

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
| `RequestFilter` | After the request is received and before backend selection | Auth, RateLimit, CORS, IP restriction |
| `UpstreamResponseFilter` | After upstream response headers are received | ResponseHeaderModifier, `Wasm` (`proxy_on_response_headers`) |
| `UpstreamResponseBodyFilter` | After upstream response body is received | ResponseRewrite (body modification), `Wasm` (`proxy_on_http_response_body`, read-only) |
| `UpstreamResponse` | After the upstream response headers arrive (async) | `ExtProc` (response-header processing); logic needing async access to the response |

## Secret Dependencies

The following plugin types read Secret data from `GLOBAL_SECRET_STORE` during the Controller-side `parse()` stage:

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

Secret changes automatically trigger reprocessing of EdgionPlugins via SecretRefManager.
