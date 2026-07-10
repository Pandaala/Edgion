# Edgion Extension Filters

HTTP Route-level plugins provided by Edgion, configured via the `EdgionPlugins` CRD.

## Contents

| Document | Description |
|----------|-------------|
| [bandwidth-limit.md](bandwidth-limit.md) | Bandwidth throttling |
| [basic-auth.md](basic-auth.md) | HTTP Basic authentication |
| [canary.md](canary.md) | Full-cutover, remotely switchable traffic canary |
| [cors.md](cors.md) | Cross-origin resource sharing (CORS) |
| [csrf.md](csrf.md) | CSRF protection |
| [ctx-setter.md](ctx-setter.md) | Context variable extraction |
| [direct-endpoint.md](direct-endpoint.md) | Direct connection to a specific endpoint |
| [dsl/README.md](dsl/README.md) | DSL scripting engine |
| [dynamic-upstream.md](dynamic-upstream.md) | Dynamic upstream selection |
| [forward-auth.md](forward-auth.md) | External authentication delegation |
| [ext-proc.md](ext-proc.md) | External processing over gRPC (Envoy ext_proc compatible) |
| [global-access-control.md](global-access-control.md) | CDN/origin lockdown: composite IP-CIDR + secret-header allow-list with profile hot-swap |
| [header-cert-auth.md](header-cert-auth.md) | Header/certificate authentication |
| [hmac-auth.md](hmac-auth.md) | HMAC signature verification |
| [ip-restriction.md](ip-restriction.md) | IP allowlist/denylist |
| [jwe-decrypt.md](jwe-decrypt.md) | JWE Token decryption |
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
| [request-restriction.md](request-restriction.md) | Request filtering |
| [response-rewrite.md](response-rewrite.md) | Response rewrite |
| [trace-context.md](trace-context.md) | W3C Trace Context propagation |
| [wasm.md](wasm.md) | Execute a proxy-wasm ABI guest module (WebAssembly) via wasmtime |
| [all-endpoint-status.md](all-endpoint-status.md) | Query all backend endpoint statuses |
| [debug-access-log.md](debug-access-log.md) | Inject access log into response headers (dev only) |
