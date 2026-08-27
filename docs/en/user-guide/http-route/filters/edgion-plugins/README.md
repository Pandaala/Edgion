# Edgion Extension Filters

HTTP Route-level plugins provided by Edgion, configured via the `EdgionPlugins` CRD.

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
| [credential-injector.md](credential-injector.md) | Secret-backed upstream bearer or custom-header credential injection |
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
| [rate-limit.md](rate-limit.md) | Rate limiting (in-memory, approximate) |
| [rate-limit-redis.md](rate-limit-redis.md) | Rate limiting (Redis, precise, cluster-wide) |
| [real-ip.md](real-ip.md) | Client real IP extraction |
| [region-route.md](region-route.md) | Region-based routing |
| [request-mirror.md](request-mirror.md) | Request mirroring |
| [request-body-buffer.md](request-body-buffer.md) | Explicitly cache a request body for later-stage consumers |
| [request-restriction.md](request-restriction.md) | Request filtering |
| [response-rewrite.md](response-rewrite.md) | Response rewrite |
| [trace-context.md](trace-context.md) | W3C Trace Context propagation |
| [wasm.md](wasm.md) | Execute a proxy-wasm ABI guest module (WebAssembly) via wasmtime |
| [all-endpoint-status.md](all-endpoint-status.md) | Query all backend endpoint statuses |
| [debug-access-log.md](debug-access-log.md) | Inject access log into response headers (dev only) |
