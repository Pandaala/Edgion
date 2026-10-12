# Edgion Extension Filters

HTTP Route-level plugins provided by Edgion, configured via the `EdgionPlugins` CRD.

## Authentication realm

BasicAuth, KeyAuth, JwtAuth, HmacAuth, LdapAuth, and OpenidConnect share the same
character policy for `config.realm`. Set the literal protection-domain name:
Edgion escapes double quotes and backslashes when writing `WWW-Authenticate`.
Do not pre-escape the value for HTTP; only use the escaping required by YAML/JSON.

ASCII control characters (`0x00`–`0x1F`, including CR, LF, NUL, and tab) and DEL
(`0x7F`) are rejected, not removed. Other characters retain their literal value.
Existing per-plugin defaults and empty-value rules are unchanged.

For example, YAML `realm: 'Partner "API"\'` produces the parameter
`realm="Partner \"API\"\\"` in the challenge.

An enabled plugin with an invalid realm reports `Accepted=False` on its
`EdgionPlugins` resource. The Gateway fails requests through that resource closed
with HTTP 500, including after an invalid hot update; applying a valid value
restores normal authentication. The validation message identifies the character
rule without echoing the realm. Disabled entries remain inert.

## Contents

| Document | Description |
|----------|-------------|
| [ai-guard.md](ai-guard.md) | Request-side prompt guarding: regex/PII rules, mask, refusal, and one bounded external evaluator |
| [ai-proxy.md](ai-proxy.md) | Rule-level AiProxy entry; full docs in [AI Gateway](../../../../ai-gateway/) |
| [bandwidth-limit.md](bandwidth-limit.md) | Bandwidth throttling |
| [basic-auth.md](basic-auth.md) | HTTP Basic authentication |
| [canary.md](canary.md) | Full-cutover, remotely switchable traffic canary |
| [conditions.md](conditions.md) | Plugin `run`/`skip` conditions (`keyExist`, `keyMatch`, `timeRange`, `probability`) |
| [cors.md](cors.md) | Cross-origin resource sharing (CORS) |
| [csrf.md](csrf.md) | CSRF protection |
| [credential-injector.md](credential-injector.md) | Secret- or LinkSys-backed upstream bearer or custom-header credential injection |
| [ctx-setter.md](ctx-setter.md) | Context variable extraction |
| [webhook-key-get.md](webhook-key-get.md) | Resolve a LinkSys webhook value into request context |
| [direct-endpoint.md](direct-endpoint.md) | Direct connection to a specific endpoint |
| [dsl/README.md](dsl/README.md) | DSL scripting engine |
| [dye.md](dye.md) | Traffic-tagging headers (entry-level `dye` — outcome vocabulary, value templates, anti-spoof strip) |
| [dynamic-upstream.md](dynamic-upstream.md) | Dynamic upstream selection |
| [forward-auth.md](forward-auth.md) | External authentication delegation |
| [form-json-transform.md](form-json-transform.md) | Convert bounded request bodies between flat form URL encoding and JSON |
| [ext-proc.md](ext-proc.md) | External processing over gRPC (Envoy ext_proc compatible) |
| [request-access-policy.md](request-access-policy.md) | L7 host/path/IP access policies with Secret/HMAC conditions and profile hot-swap |
| [guardrail.md](guardrail.md) | Bounded unary Pass/Replace/Reject decisions for canonical semantic AI SSE windows |
| [grpc-json-transcoder.md](grpc-json-transcoder.md) | Descriptor-backed REST JSON to native gRPC over mandatory upstream HTTP/2 |
| [rest-to-graphql.md](rest-to-graphql.md) | Static GraphQL request adaptation with shared KeyGet variable bindings |
| [graphql-operation-router.md](graphql-operation-router.md) | Exact operation selection among declared HTTPRoute Service backends |
| [graphql-rate-limit.md](graphql-rate-limit.md) | Static SDL query-cost budgets through Redis |
| [graphql-guard.md](graphql-guard.md) | Independent shape and introspection protection with optional SDL cost ceiling |
| [graphql-safelist.md](graphql-safelist.md) | Static trusted GraphQL documents |
| [header-cert-auth.md](header-cert-auth.md) | Header/certificate authentication |
| [hmac-auth.md](hmac-auth.md) | HMAC signature verification |
| [ip-restriction.md](ip-restriction.md) | IP allowlist/denylist |
| [jwe-decrypt.md](jwe-decrypt.md) | JWE Token decryption |
| [json-schema-validation.md](json-schema-validation.md) | Validate JSON request bodies against an inline JSON Schema |
| [jwt-auth.md](jwt-auth.md) | JWT authentication |
| [key-auth.md](key-auth.md) | API Key authentication |
| [ldap-auth.md](ldap-auth.md) | LDAP authentication |
| [mock.md](mock.md) | Mock response |
| [openid-connect.md](openid-connect.md) | OpenID Connect authentication |
| [proxy-rewrite.md](proxy-rewrite.md) | Proxy rewrite |
| [rate-limit-local.md](rate-limit-local.md) | Rate limiting (in-memory, approximate) |
| [rate-limit-redis.md](rate-limit-redis.md) | Rate limiting (Redis, precise, cluster-wide) |
| [real-ip.md](real-ip.md) | Client real IP extraction |
| [region-route.md](region-route.md) | Region-based routing |
| [request-mirror.md](request-mirror.md) | Request mirroring |
| [request-body-buffer.md](request-body-buffer.md) | Explicitly cache a request body for later-stage consumers |
| [request-restriction.md](request-restriction.md) | Request filtering |
| [response-rewrite.md](response-rewrite.md) | Response rewrite |
| [request-id.md](request-id.md) | RequestId and default W3C context |
| [wasm.md](wasm.md) | Execute a proxy-wasm ABI guest module (WebAssembly) via wasmtime |
| [waf.md](waf.md) | Native bounded SecLang WAF spanning the supported HTTP transaction phases |
| [all-endpoint-status.md](all-endpoint-status.md) | Query all backend endpoint statuses |
| [debug-access-log.md](debug-access-log.md) | Inject access log into response headers (dev only) |
