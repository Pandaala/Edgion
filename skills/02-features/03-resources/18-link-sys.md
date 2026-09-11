---
name: link-sys-detail
description: Full LinkSys CRD schema: Redis/Etcd/Elasticsearch/Webhook/Kafka/HttpDns connectors and Controller-managed credential sources with a unified secretRef/TLS credential model.
---

# LinkSys Resource

> API: `edgion.io/v1` | Scope: Namespaced | ShortName: `lsys`

LinkSys is the unified external-system connector resource. The top-level `spec.type` distinguishes the backend, and `spec.config` carries the detailed configuration for that backend. Operator-supplied credentials go through `secretRef` referencing K8s `Secret`; **inlining** sensitive fields such as username, password, token, or `clientKey` in the CRD is **not allowed** (they are hard-rejected at deserialization by `deny_unknown_fields`). `type: credentialSource` is Controller-managed token acquisition: bootstrap client secrets still use `secretRef`, but generated access tokens are published either as a managed Secret (`publication.persist: true`) or a Controller-only memory map (`persist: false`).

## Top-level Schema

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: my-redis
  namespace: default
spec:
  type: redis                 # redis | etcd | elasticsearch | webhook | kafka | httpdns | credentialSource
  config:                     # Backend-type-specific configuration (expanded by type below)
    ...
```

`type` and `config` form a tagged enum with `#[serde(tag = "type", content = "config")]`: `type: redis` in YAML corresponds to `RedisClientConfig`, `type: webhook` to `WebhookServiceConfig`, and so on. The older form that placed `type` inside `spec.config.type` is no longer supported.

## Credential Model (Common)

### Secret Reference: `SecretObjectReference`

```yaml
secretRef:
  name: my-secret           # Required
  namespace: credentials    # Optional; defaults to the LinkSys namespace
  # group / kind may be omitted; default is core/Secret
  # Reserved fields, kept for future support of non-Secret credential sources (Vault / ExternalSecret)
```

- Cross-namespace (`namespace != current resource namespace`) requires a Gateway API `ReferenceGrant` permitting `Secret` in the target namespace; otherwise `ResolvedRefs=False/RefNotPermitted`.
- Secret keys follow the **well-known convention** (table below); the CRD does not support custom fields like `usernameKey` / `passwordKey`.
- The underlying type is `SecretObjectReference` (`group / kind / name / namespace`); `group` and `kind` default to `core/Secret`, retaining the ability to extend to non-Secret credential sources (Vault / ExternalSecret) in the future.

### Secret Key Convention (aligned with the resolver)

| Auth Type | Recommended Secret.type | Required Keys | Optional Keys |
|-----------|--------------------------|----------------|----------------|
| basic (Redis/Etcd/ES) | `kubernetes.io/basic-auth` or `Opaque` | `password` | `username` (default = backend default ACL) |
| apiKey (Elasticsearch) | `Opaque` | `apiKeyId`, `apiKeySecret` | — |
| bearer (Elasticsearch) | `Opaque` | `bearerToken` | — |
| TLS (all providers) | `kubernetes.io/tls` or `Opaque` | **At least one of** `tls.crt` / `tls.key` / `ca.crt` | — |

> **Webhook secrets** no longer use the well-known-key convention. After the webhook
> redesign, webhook outbound credentials are arbitrary `request.headers.custom` entries
> using a `template: "<prefix>${secretRef:ns/name:key}"` source. The controller resolves
> the referenced Secret at config-apply time and ships the resolved value inline in
> `spec.resolved_secrets`; the gateway substitutes it at call time. Bearer/HMAC/apiKey
> are expressed as `Authorization` / custom headers built from this template form (e.g.,
> `template: "Bearer ${secretRef:prod/webhook-token:token}"`). See the `type: webhook`
> section below.

All application-layer key names use **camelCase**; TLS keys follow the K8s `kubernetes.io/tls` standard (which contains dots).

> **Breaking change notice**: all `*Auth` structs have `deny_unknown_fields` enabled. Writing `username: ...` / `password: ...` / `bearerToken: ...` / `apiKeyId: ...` / `clientKey: ...` directly in the CRD YAML causes parsing to fail (it is not silently ignored).

### TLS Credential Structure

```yaml
tls:
  enabled: true
  verify: true                    # Verify peer certificate (default true). Redis/Etcd/Kafka always verify (rustls connector; verify: false is rejected at config load). Other connectors (Elasticsearch/Webhook) honor verify: false as operator responsibility (WARN logged at startup).
  validation:                     # Optional; omit to use built-in webpki root CAs (same as wellKnownCACertificates: System).
    caCertificateRefs:            # List of CA certificate refs. For LinkSys (redis/etcd/es/kafka), only kind: Secret is supported in this release.
      - kind: Secret              # Secret must be kubernetes.io/tls or Opaque with a ca.crt key.
        name: my-ca-bundle        # Defaults to the same namespace as LinkSys when namespace is omitted.
    wellKnownCACertificates: System   # Alternatively, explicitly use the built-in webpki root bundle.
    # subjectAltNames (SAN pinning) is enforced for OIDC and LDAP; deferred for
    # LinkSys (redis/etcd/elasticsearch/kafka), ext_proc, and EGC defaults.
    # hostname (SNI override) is not implemented on any outbound provider yet.
    # Configuring an unenforced field causes Accepted=False (no silent no-op).
    # subjectAltNames: ["backend.example.com"]
    # hostname: "backend.example.com"   # currently rejected everywhere
  clientCertificateRef:           # Secret holding tls.crt + tls.key for client mTLS
    name: my-client-tls
    # Defaults to the same namespace as LinkSys when namespace is omitted.
```

- The client identity (certificate + private key) **must** be provided via `clientCertificateRef`, pointing to a `kubernetes.io/tls` or `Opaque` Secret with `tls.crt` and `tls.key` keys.
- CA trust anchors come from `validation.caCertificateRefs` (Secret refs; for LinkSys only `kind: Secret` is supported) or `validation.wellKnownCACertificates: System` (built-in webpki roots). A custom CA **replaces** the public roots. Inline CA (`caCert`) has been removed in this release; use a `kubernetes.io/tls` or `Opaque` Secret with a `ca.crt` key instead.
- For providers whose URLs already imply TLS (`https://`, `rediss://`), `enabled: true` only additionally enables custom CA / client certificate handling; `tls` may be omitted when only system CAs are used.

> **Phase 2 outbound TLS status:**
> - `validation.caCertificateRefs` for LinkSys (redis / etcd / elasticsearch / kafka) only supports `kind: Secret` in this release. ConfigMap CA is not supported for LinkSys. If you need ConfigMap CA support, use the plugin (OIDC / LDAP / ForwardAuth / EGC) TLS paths instead.
> - `validation.subjectAltNames` (SAN pinning) is **enforced for OIDC and LDAP plugins**. TLS handshakes use rustls `OutboundSanVerifier` to validate peer certificate SANs at runtime; mismatch results in handshake failure with the `OutboundTlsSanMismatch` error diagnostic. A non-empty `subjectAltNames` policy also **overrides `verify: false`** (fail-secure: full chain verification is forced, and the client mTLS identity is retained). **For LinkSys (redis / etcd / elasticsearch / kafka), ext_proc, and global EdgionGatewayConfig defaults, SAN enforcement is deferred to Phase 3.** Configuring `subjectAltNames` on an unsupported backend causes `Accepted=False`.
> - `validation.hostname` (SNI override) is **not implemented on any outbound provider yet** — setting it causes `Accepted=False` on every provider (including OIDC/LDAP) rather than being silently ignored. Use `subjectAltNames` for SAN pinning. See `docs/superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md` §6 and `tasks/todo/outbound-tls-sdk-san-audit.md` for the enforcement timeline.
> - Inline CA (`caCert` field) has been removed. Use a Secret with a `ca.crt` key instead.

## Configuration by `type`

### Disabled TLS blocks

An absent or false `tls.enabled` makes the local TLS policy inert: Controller
SAN/hostname checks and Secret resolution ignore it, and Gateway TLS readiness
does not wait for its certificate references. Reconciliation clears previously
resolved TLS Secrets and dependencies. Authentication references are independent
and still required. Re-enabling TLS restores all reference and validation checks.
For URL-driven transports (`https://`, `rediss://`), disabling this local block
does not disable the secure scheme. This also applies to `credentialSource` TLS.

### `type: redis`

```yaml
spec:
  type: redis
  config:
    endpoints:                    # Required, at least one
      - "redis://redis:6379"
    db: 0                         # 0-15, Redis standard database index
    auth:                         # Optional; secretRef is required when present
      secretRef:
        name: redis-auth
    pool:
      size: 16
      minIdle: 2
    timeout:
      connect: 3000               # ms
      read: 2000
      write: 2000
    topology:                     # standalone (default) | sentinel | cluster
      mode: sentinel
      sentinel:
        masterName: mymaster
        sentinels: ["sentinel-1:26379", "sentinel-2:26379"]
      # cluster:
      #   readFromReplicas: true
      #   maxRedirects: 6
    tls:
      enabled: true
      clientCertificateRef: { name: redis-client-tls }
    observability:
      logging: { enabled: true, level: info, slowLogThreshold: 200 }
      metrics: { enabled: true, labels: { env: prod } }
```

- When `topology.mode=sentinel`, `sentinel` is required; when `mode=cluster`, `cluster` is required; otherwise `Accepted=False/Invalid`.
- `auth` uses `BasicAuthResolver`: the Secret must provide at least `password`; `username` is optional (Redis 6+ ACL).

### `type: etcd`

```yaml
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1:2379"
      - "https://etcd-2:2379"
    auth:                         # Optional
      secretRef:
        name: etcd-creds
        namespace: credentials
    tls:
      enabled: true
      clientCertificateRef: { name: etcd-client-tls }
    timeout:                      # ms
      dial: 5000
      request: 10000
      keepAlive: 30000
    keepAlive:                    # HTTP/2 keepalive, in seconds
      time: 30
      timeout: 10
      permitWithoutStream: true
    namespace: "/my-app/"         # etcd v3 key prefix
    autoSyncInterval: 300         # seconds; 0 disables it
    maxCallSendSize: 2097152
    maxCallRecvSize: 4194304
    userAgent: "edgion-gateway/v1"
    rejectOldCluster: true
    observability:
      logging: { enabled: true, level: info }
      metrics: { enabled: true, labels: { env: prod } }
```

- `auth` likewise uses `BasicAuthResolver` (username + password, both typically required for etcd).
- Both Redis and Etcd clients always verify (`verify: false` is rejected at startup — the SDK has no skip-verify API). For a private CA, use `validation.caCertificateRefs` pointing to a Secret with a `ca.crt` key.

### `type: elasticsearch`

```yaml
spec:
  type: elasticsearch
  config:
    endpoints:
      - "https://es-1:9200"
      - "https://es-2:9200"
    auth:                         # Tagged enum; type is required
      type: apiKey                # basic | apiKey | bearer
      secretRef:
        name: es-api-key
    tls:
      enabled: true
    timeout:
      connect: 5000
      request: 30000
    pool:
      maxIdlePerHost: 10
      idleTimeout: 90000          # ms
    bulk:
      batchSize: 500
      flushInterval: 5000
      maxRetries: 3
      backoffMs: 1000
      maxBodyBytes: 10485760      # 10 MiB
      channelSize: 10000          # producer→bulk-loop mpsc capacity, ceiling 100000
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    compatibility:
      vendor: elasticsearch       # elasticsearch | opensearch
```

- Auth tagged-enum: `type: basic` → `BasicAuthResolver`, `type: apiKey` → `EsApiKeyResolver`, `type: bearer` → `BearerResolver`.
- At runtime, requests are issued via reqwest directly over HTTP, without depending on the official `elasticsearch` crate; OpenSearch-compatible.
- Failed-cache spill/replay is planned but is not an accepted YAML field. Do not put it in a
  LinkSys resource until both the Rust schema and CRD declare it.

### `type: webhook`

> Redesigned: `target` is either a flat Kubernetes Service reference or an origin-only
> external `url` (mutually exclusive), plus a `request` block that is the single source of
> truth for the outbound request. The architecture, controller→conf_sync→gateway flow,
> consumer `path_override` API, template engine, presence and health semantics live in
> [01-architecture/03-controller-gateway-link/webhook.md](../../01-architecture/03-controller-gateway-link/webhook.md).
> The old `uri` / `requestMethod` / `auth` / `defaultHeaders` fields and the consumer-side
> `forwardHeaders` / `pathSuffix` are **removed** (see Migration in that doc).
>
> **Breaking:** `successStatusCodes` is likewise gone, replaced by the nested `success`
> block below. It has no runtime compatibility shim: serde ignores it and success falls back to
> "any 2xx". The shared advisory unknown-field detector reports the exact path through warnings,
> status, diagnostics/metrics, and Admin API responses without changing `Accepted` or blocking
> sync. **`allowDegradation` (fail-open mode)
> and `allowDegradationTemplate` have been removed.** Forward auth now always fails closed with
> `statusOnError` (default 503) when the auth service is unreachable. The webhook connector never
> used these fields anyway — they are inert when set on a webhook LinkSys.

```yaml
spec:
  type: webhook
  config:
    target:                                        # Kubernetes Service form (fields are flat — no backendRef/uri wrapper)
      group: ""                                  # default core
      kind: Service                              # Service | ServiceClusterIp | ServiceEndpointSlice | empty
      name: webhook-svc
      namespace: prod                            # optional; defaults to LinkSys namespace
      port: 8080                                 # required — missing port is rejected (Accepted=False)

    # target:                                      # External URL form (mutually exclusive with the Service fields)
    #   url: https://api.example.com                # origin-only: scheme://host[:port], no path/query/fragment/credentials
    #   blockPrivate: true                           # optional, url targets only; default false (inherits gateway-local default)

    request:                                       # required — declares the outbound request shape
      path:                                        # omit entirely → reuse frozen incoming path
        template: "/resolve/${ctx:action}"         # literal or ${type:name} template
        allowOverride: true                        # consumer pathOverride honored only when true
      method:                                      # omit → reuse frozen incoming method
        template: "POST"
      args:                                        # allow-list; nothing forwarded unless named
        origin:
          - { name: tenant_id }                    # missing → skipIfMissing (default)
          - { name: trace_id, presence: required } # missing → webhook call fails
          - { name: org_id, presence: warnIfMissing }
        custom:
          - { name: source, value: "edgion" }      # literal
          - { name: action, template: "${ctx:action}" }
      headers:
        origin:
          - { name: X-Tenant-Id }
        custom:
          - { name: X-Caller, value: "edgion-gateway" }
          - name: Authorization                    # secretRef template: controller-resolved + carried inline
            template: "Bearer ${secretRef:prod/webhook-token:token}"
      cookies:
        origin:
          - { name: session_id }
      body:                                       # optional — omit to send no body
        type: json                                 # json | form | raw
        template: |                                # json/raw: ${type:name} template
          {"userId": "${ctx:userId}", "token": "${secretRef:prod/webhook:token}"}
        # form uses structured fields instead of a template (gateway percent-encodes):
        # type: form
        # fields:
        #   - name: userId
        #     template: "${ctx:userId}"
        #   - name: source
        #     value: edgion

    timeoutMs: 3000
    timeoutMsTemplate: "${ctx:desired_timeout_ms}"   # optional; only ctx/remoteIp/matchedRoute vars allowed (see below); falls back to timeoutMs; rendered value clamped to 60000ms
    tls:
      enabled: true
      clientCertificateRef: { name: webhook-client-tls }
    success:                                       # replaces the removed `successStatusCodes` (see the note above)
      statusCodes: [200]                           # omitted -> any 2xx
      body:                                        # optional; ANDed with statusCodes
        - pointer: /code
          equals: 0
    maxResponseBytes: 4096                         # must be >= 4096 when success.body is set; clamped to EdgionGatewayConfig.linkSys.webhookMaxResponseBytes (default 16 KiB)
    # statusOnError is accepted here for schema compatibility with forward_auth (same
    # ExternalHttpConnection struct) but is NOT read by the webhook connector: WebhookKeyGet
    # writes no value (or `default`) on failure, full stop. See below.
    retry:
      maxRetries: 1              # range 0..=10 (admission ceiling)
      retryDelayMs: 100          # base delay of the backoff curve, not a fixed interval
      backoffPolicy: exponential # exponential (default) | fixed
      maxDelayMs: 1000           # per-attempt ceiling, range 1..=10000
      jitter: true               # 50-100% jitter, applied after the maxDelayMs cap
      honorRetryAfter: false     # opt-in; only for 429/503 already in retryOnStatus
      retryOnTimeout: false      # webhook default false (forward_auth default true; see below)
      retryOnConnectError: true
      retryOnStatus: [502, 503, 504]
      retryOnBodyFailure: false  # retry when a success.body predicate fails
    rateLimit:
      rate: 200
      windowSec: 1
    healthCheck:
      active:
        path: "/healthz"
        intervalSec: 10
        timeoutMs: 2000
        healthyThreshold: 1
        unhealthyThreshold: 3
      passive:
        unhealthyThreshold: 3
        failureStatusCodes: [500, 502, 503, 504]
        countTimeout: true
```

**Schema reference:**

- **`target`** (`WebhookTarget`): a flat struct with two mutually-exclusive forms, enforced by `WebhookTarget::kind()` — setting `url` together with any Service field (`name`/`namespace`/`port`/`kind`/`group`) is an admission error, and setting neither also errors.
  - **Service form** (`name`/`namespace`/`port`/`kind`/`group` directly under `target`, no nesting): required `name` + `port`; `kind` ∈ `{Service, ServiceClusterIp, ServiceEndpointSlice, "", unset→Service}`. `ExternalName` / `ServiceImport` are rejected at admission. `Service`/`ServiceEndpointSlice` resolve ready endpoints from `EpSliceStore` at call time; `ServiceClusterIp` uses ClusterIP DNS.
  - **URL form** (`url` + optional `blockPrivate`): `url` must be origin-only (`scheme://host[:port]`, `http`/`https` only, no path/query/fragment/credentials) — a path/query/fragment/credentials or a non-http(s) scheme is rejected (`Accepted=False`, message contains `origin-only`); the request path comes from `request.path.template`. `http://` combined with `tls.enabled: true` is also rejected (no silent plaintext downgrade). The target is SSRF-guarded: loopback/link-local/unspecified addresses are always blocked; `blockPrivate: true` additionally blocks RFC1918 + unique-local ranges (default `false`, inherits the gateway-local `outbound_egress_block_private` default). A URL target always gets its own dedicated connection pool + DNS resolver.
- **`request.path`** (`PathSpec`): `{ template: String, allowOverride: bool = false }`. Omit the whole block → reuse the frozen incoming path.
- **`request.method`** (`MethodSpec`): `{ template: String }`. Omit → reuse the frozen incoming method.
- **`request.{args,headers,cookies}`** (`ListFields`): `{ origin: [OriginField], custom: [CustomField] }`, both default empty. An allow-list — only named fields appear on the wire. Cookies are emitted as a combined `Cookie` header.
- **`OriginField`**: `{ name: String, presence: PresencePolicy = skipIfMissing }`. Copies `name` from the **frozen** entry-time incoming request.
- **`PresencePolicy`** enum (camelCase): `skipIfMissing` (default — omit, no log), `warnIfMissing` (omit + PluginLog alert + continue), `required` (treat as webhook failure: WebhookKeyGet writes no value / `default`; webhook connector does not read `statusOnError` — see the note below).
- **`CustomField`**: `{ name: String, <source> }` where source is exactly one of (`CustomSource`, externally tagged + flattened):
  - `value: "literal"` → verbatim string.
  - `template: "${ctx:x}/${header:y}"` → ctx-set template, evaluated at call time. Secret values are embedded as `${secretRef:ns/name:key}` inside the template string (e.g., `"Bearer ${secretRef:prod/webhook-token:token}"`); the controller resolves the referenced Secret at config-apply time, embeds the values in `resolved_secrets`, and ships the full struct over conf_sync; the gateway substitutes `${secretRef:...}` vars from the carried map at call time. Admin `/configclient` renders resolved secret values as `"[redacted]"`. Both `ns` and `key` are required in the `secretRef` token.
- **`request.body`** (`BodySpec`, tagged enum on `type`): optional — omit the whole block to send no body.
  - `type: json`: `template` (`${type:name}` interpolation). Every variable value is JSON-string-escaped at eval time, so it always renders as (part of) a JSON string. A config-time canary lint substitutes a placeholder for every variable and parses the result as JSON; a template whose skeleton isn't valid JSON is rejected at admission — this also catches an unquoted placeholder like `"count": ${ctx:n}`. Numeric/boolean payload fields need `type: raw`.
  - `type: form`: `fields: [CustomField]` (same `value`/`template` source as `headers.custom`), sent as `application/x-www-form-urlencoded` with each value percent-encoded by the gateway.
  - `type: raw`: `template`, sent verbatim after interpolation. No Content-Type is auto-injected — set one via `headers.custom` if needed.
  - `json`/`form` auto-inject `Content-Type` (`application/json` / `application/x-www-form-urlencoded`) after header assembly, so it overrides any incoming value forwarded via `headers.forwardAll`. An explicit `Content-Type` in `headers.origin`/`headers.custom` combined with `type: json`/`form` is rejected at admission.
  - `${secretRef:ns/name:key}` works in body templates/fields exactly as in `headers.custom`: controller-resolved and redacted (`"[redacted]"`) in admin output.
  - Rendered body size is **not** capped — a template referencing a ctx variable of unbounded size flows straight through; keep templates away from unaudited large inputs.
  - The `request` block, including `body`, is shared with the `forward_auth` plugin's outbound request.
- **`success`** (`SuccessSpec`, replaces the removed `successStatusCodes`): `{ statusCodes: [u16], body: [BodyPredicate] }`, both optional. `statusCodes` omitted → any 2xx; an explicit empty list (`[]`) is rejected at admission rather than silently meaning the same thing. `BodyPredicate`'s operator field (`equals`/`notEquals`/`exists`/`in`) has a custom `Deserialize` that enforces **exactly one** operator key at parse time — two operators (or zero) fails to deserialize, so `{pointer: /code, exists: true, equals: 0}` cannot silently keep only the first-matching key. Predicates are ANDed with the status check and with each other; matched as strict `serde_json::Value` (no coercion — `equals: 0` ≠ `"0"`). A non-JSON body is a `BodyFail` (not an error) when any `body` predicate is configured. A missing `pointer` is also a `BodyFail` for `equals`/`notEquals`/`in` — you cannot assert an equality/membership claim about a field that isn't there — with `exists: false` as the one operator defined to succeed on a missing pointer. Admission requires `maxResponseBytes >= 4096` whenever `success.body` is non-empty; a body truncated by the cap surfaces as `ResponseTooLarge`, not a failed predicate. Passive health (`healthCheck.passive.failureStatusCodes`) still keys off status only — a `success.body` failure never trips the circuit.
- **`retry`** (`WebhookRetry`) gained `backoffPolicy` (`exponential` default | `fixed`), `maxDelayMs` (default 1000, admission ceiling 10000), `jitter` (default true, 50-100% band applied after the cap), `honorRetryAfter` (default false; only takes effect for a 429/503 already listed in `retryOnStatus`; a `Retry-After` above `maxDelayMs` stops the retry loop and returns the response as-is rather than erroring), and `retryOnBodyFailure` (default false; retries a `success.body` predicate failure). `retryDelayMs` is the **base delay of the backoff curve** (`delay = min(maxDelayMs, retryDelayMs << (attempt-1))` under `exponential`), not a fixed interval. **`retryOnTimeout`'s default is now per-consumer**: webhook `false`, `forward_auth` `true` — a connect failure is always safe to retry either way, so `retryOnConnectError` stays `true` for both; only the timeout default is split (webhook POSTs are typically non-idempotent, forward_auth checks are idempotent).
- **`timeoutMsTemplate`**: optional per-request template on the same `${type:name}` engine as `request.*`, but restricted to a **much narrower variable allow-list**: only `${ctx:...}`, `${remoteIp}`, `${matchedHttpRoute:...}`, `${matchedGrpcRoute:...}`. `${header:...}`/`${query:...}`/`${cookie:...}`/`${path}`/`${uri}`/`${requestPath}`/`${requestUri}`/`${method}`/`${secretRef:...}` are all rejected at admission specifically for this field (they remain fully allowed in `request.*`) — it is security-sensitive (call deadline), so it may not be driven by anything the client controls on the request. It is compiled and validated at admission (a malformed template, or one using a disallowed variable type, is `Accepted=False`). `timeoutMsTemplate` parses as `u64` ms, falls back to `timeoutMs` on a non-numeric/empty/below-floor render (floor: `MIN_OUTBOUND_TIMEOUT_MS` = 50ms), and is clamped to 60000ms on the upper end regardless of the render. Every silent fallback increments `edgion_outbound_template_fallback_total{field="timeout_ms"}` (data-plane observability goes through Prometheus, not `tracing`). A `success.body` predicate that fails a 2xx-status response increments `edgion_outbound_success_predicate_fail_total`. **Note:** `statusOnError` is part of the shared `ExternalHttpConnection` schema but is honored by `forward_auth` only — WebhookKeyGet has no fail-close/fail-open decision to make and simply writes no value (or `default`) on any failure; this field is inert when set on a webhook `LinkSys`.
- **Validation** (`Accepted=False` on violation): missing `port`; unsupported `kind`; template parse error, or a client-controlled variable type, in `timeoutMsTemplate` (`request.*` templates keep the full variable set); same name in both `origin` and `custom`; duplicate name within a block; `headers.origin: [{name: Cookie}]` together with a non-empty `cookies` block; `request.body` type `json` whose canary-rendered template does not parse as valid JSON; an explicit `Content-Type` header combined with `request.body` type `json`/`form`; `retry.retryDelayMs == 0`; `retry.maxDelayMs` outside `1..=10000`; `retry.maxRetries` outside `0..=10`; `timeoutMs == 0`; `maxResponseBytes == 0`; `success.statusCodes == []` (explicit empty list); `success.body` non-empty with `maxResponseBytes < 4096`; a `success.body` predicate with zero or more than one operator key (rejected at deserialize time, before this validation function even runs).
- **`statusOnError`** ∈ [200, 599] (validated, but inert for webhook — see above); `maxResponseBytes` clamped to `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes`.

**Consumer side:** the outbound request shape is owned entirely by the webhook resource. The operator-facing consumer is the `WebhookKeyGet` plugin (`WebhookVar`: `webhookRef`, optional `pathOverride`, `extract`, `default`). It passes only the `session` and an optional `pathOverride` (honored only when `path.allowOverride: true`); the `forwardHeaders` / `pathSuffix` knobs are gone. Per-call dynamic values flow in via a preceding CtxSet entry → `${ctx:NAME}` in the webhook templates.

**Removed-field debt:** current code still accepts `auth` / `defaultHeaders` /
`requestMethod` as ignored no-ops and logs a warning. This is not a supported
rolling-upgrade contract; it conflicts with the repository's pre-release
no-compatibility policy. Do not use or extend these fields. Remove the
placeholders, warning, and associated tests together when code changes are in
scope. `successStatusCodes` is ignored at runtime but reported by the advisory
unknown-field detector; see the note at the top of this section.

### `type: kafka`

```yaml
spec:
  type: kafka
  config:
    brokers:
      - "kafka:9092"
```

- Kafka has a live Gateway provider, runtime store, and full/partial update lifecycle.

### `type: httpdns`

An HTTP DNS resolver: resolves external domains via an HTTP(S) endpoint instead of standard DNS. The `HttpDnsResolverConfig` schema and the shared resolver core live in `edgion-common/src/dns/`. The gateway data-plane wiring is live: a `ServiceExternalName` backend whose effective resolver (Service → Gateway → EGC precedence) points at an httpdns LinkSys is resolved through this config, cached briefly, and SSRF-validated. See [`docs/superpowers/specs/2026-07-02-external-dns-resolution-design.md`](../../../docs/superpowers/specs/2026-07-02-external-dns-resolution-design.md) for the full design.

Key fields:
- `preset` (`aliyun` | `tencent`) — selects the response **format** (Aliyun JSON `ips`; Tencent DNSPod `;`-delimited) plus a best-effort built-in URL. The built-in URLs are plain HTTP against the vendors' public resolving IPs and are account-less, so for real vendor accounts combine `preset` with an explicit `urlTemplate`.
- `urlTemplate` — request URL with a `{domain}` placeholder. **Wins over a preset's built-in URL**, so `preset` + `urlTemplate` = the preset's response format with a custom (account-scoped / correct-scheme) endpoint. Required only when no `preset` is set.
- `response` — parsing rules (`json` with `ipPath`, or `delimited` with `delimiter`); ignored when a `preset` is set.
- `connection` — lean `{ tls, timeoutMs }` (no `target`); outbound TLS + request timeout for the fetch.
- `fallback` — `system` | `dns` (server list) | `none`, applied on fetch/parse failure.

### `type: credentialSource`

Controller-managed credential acquisition (currently OAuth2 client credentials).
The Gateway treats this variant as metadata-only and never contacts the issuer.
Consumers reference it from `CredentialInjector` via
`credentialRef: { group: edgion.io, kind: LinkSys, name: <source> }`.

```yaml
spec:
  type: credentialSource
  config:
    provider:
      type: oauth2ClientCredentials
      tokenEndpoint: https://issuer.example.com/oauth2/token
      clientAuthentication:
        method: clientSecretBasic
        activeSecretRef:
          name: oauth-client            # Opaque Secret with clientId + clientSecret
        # previousSecretRef:            # optional fallback bootstrap
        #   name: oauth-client-previous
      scopes: ["inference.invoke"]
      tls:
        enabled: true
    rotation:
      interval: 15m                     # default 15m
      requestTimeout: 10s
    egress:
      blockPrivate: true                # default true
    publication:
      persist: true                     # default true (generated Kubernetes Secret)
      memoryMaxKeys: 10000              # default 10000; range 1..=10000
```

**`publication`:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `persist` | boolean | `true` | `true` writes a generated Secret named after the consuming `EdgionPlugins` (Kubernetes-only); outside Kubernetes it is `Accepted=False` / `UnsupportedConfigurationCenter`. `false` keeps generations in a Controller-local memory map: no generated Secret, allowed on leader-only Kubernetes and FS/etcd. Kubernetes `ha_mode: all-serve` + `persist: false` is `Accepted=False` / `UnsupportedHaMode`. |
| `memoryMaxKeys` | integer | `10000` | Cap on that per-LinkSys map, range `1..=10000`, validated even when `persist: true`; the cap cannot be raised above `10000`. OAuth2 uses at most two entries (active + previous). |

The key map is **Controller-only**. It is never a LinkSys spec field, never on
LinkSys conf_sync, never on LinkSys status, and never on Admin LinkSys
`/configclient`. The active token is still copied onto the consuming
EdgionPlugins `resolvedCredential` (Admin redacts that value).

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted              # Whether the spec is structurally valid + per-provider validate
      status: "True"
      reason: Accepted
    - type: ResolvedRefs          # Whether all secretRef / tls.secretRef have been successfully resolved
      status: "True"
      reason: ResolvedRefs
    - type: UnknownConfigFields   # Present only when an unrecognized typed config key was submitted
      status: "True"
      reason: UnknownFieldsDetected
  credentialState: Valid          # credentialSource only: Idle | Valid | Expired | Unavailable
```

**`Accepted`**:
- `True/Accepted` — structure + `validate_config()` both passed.
- `False/Invalid` — has validation errors (empty endpoints, missing Sentinel/Cluster sub-config, invalid Webhook URI, etc.); `message` concatenates the errors.
- `False/UnsupportedConfigurationCenter` — `credentialSource` uses `persist: true` outside Kubernetes.
- `False/UnsupportedHaMode` — `credentialSource` uses `persist: false` when the Controller topology does not have a single publisher, currently Kubernetes `ha_mode: all-serve`.

**`ResolvedRefs`**:
- `True/ResolvedRefs` — all referenced `Secret`s have been found, `Secret.type` is valid, and all required keys exist and are decodable.
- `False/SecretNotFound` — the Secret has not yet appeared in the apiserver (the Controller automatically requeues on Secret creation events).
- `False/RefNotPermitted` — cross-namespace, no matching `ReferenceGrant`.
- `False/InvalidSecretType` — `Secret.type` is not in the resolver's allowlist (e.g., basic auth received `kubernetes.io/dockerconfigjson`).
- `False/MissingKeys` — the Secret is missing required keys (e.g., basic missing `password`, apiKey missing `apiKeyId` + `apiKeySecret`).
- `False/DecodeError` — UTF-8 decoding or JSON parsing of a key failed (only ACME private keys go through JSON validation; LinkSys essentially never hits this).

**`UnknownConfigFields`**:
- Present as `True/UnknownFieldsDetected` when a typed Redis, Etcd, Elasticsearch, Kafka, Webhook,
  HttpDns, or credentialSource configuration contains an unrecognized key.
- Advisory only: it does not change `Accepted` or `ResolvedRefs` and never blocks synchronization.
- Removed after the resource is reapplied without unknown keys.
- Declared maps and arbitrary-JSON values are intentionally open below their boundary; an unknown
  sibling alongside them is still reported.

`kubectl get linksys` directly displays the `Accepted` / `ResolvedRefs` columns.

**`credentialState`** (credentialSource only, camelCase): `Idle` (no consumer),
`Valid` (an active generation is published), `Expired`, or `Unavailable`. It
does not carry the token.

## Runtime Behavior

- **Controller side for Gateway-client providers**: parse `secretRef` → register an owner→secret dependency in `SecretRefManager` (Secret update/delete cascades requeue this resource) → embed the entire `Secret` into `spec.config.<auth>.secret` / `spec.config.tls.certs.secret` → sync to the Gateway via conf_sync gRPC. The `Secret` itself is a `no_sync_kind` and is not synced separately.
- **Gateway side**: after `LinkSysStore` receives the spec, it uses the resolvers in `edgion-resources/src/credentials` to recompute `Material` from the embedded `Secret.data` and hands it to the Redis / Etcd / ES / Webhook client to build the connection. Of the four credential states (`NotRequired` / `Pending` / `Invalid` / `Resolved`), only the former or `Resolved` allows building a client; the rest fail-fast and **do not** degrade to anonymous connections.
- **Hot reload**: when a Secret pointed to by secretRef changes → the Controller recomputes → pushes the new spec via conf_sync → the Gateway rebuilds the corresponding provider client and the old connections drain gracefully.
- **`credentialSource`**: the Controller credential manager acquires and rotates the token. Persist-on writes a generated Secret; persist-off stores the map on `ControllerRuntime.credential_memory`. LinkSys conf_sync never carries that map. The Gateway `LinkSysStore` is a no-op for this variant; injection uses EdgionPlugins `resolvedCredential`.
- **Publication mode changes**: persist-on to persist-off establishes a live memory credential before owned generated Secrets are deleted, and failed deletes are retried. Persist-off to persist-on keeps memory until every current consumer Secret is consistent.
- **Ephemeral teardown**: leadership loss, Controller runtime reset/restart, LinkSys delete or type change, LinkSys UID replacement, last-consumer removal, and wipe-all clear the memory slot. The manager reacquires on the next active reconcile. A Gateway may continue using a generation already embedded in its PluginRuntime until `validUntil`; requests fail closed when no unexpired resolved credential remains.
- **Consumer limit**: one EdgionPlugins resource may contain at most one enabled LinkSys-backed CredentialInjector. Static Secret-backed injectors do not count toward this limit.

## Cross-references and Dependencies

| Reference Direction | Target Resource | Mechanism | Typical Scenario |
|---------------------|-----------------|-----------|------------------|
| LinkSys → Secret | `Secret` | `secretRef` / `tls.certs.secretRef` + `SecretRefManager` | All auth / mTLS credentials |
| LinkSys → ReferenceGrant | `ReferenceGrant` | Authorization for cross-namespace `secretRef` | Centralize credentials in a `credentials` namespace |
| AccessLog → LinkSys | `LinkSys` (type=elasticsearch/redis/webhook) | Reference by `namespace/name` in configuration | Log output to ES / Redis |
| EdgionPlugins → LinkSys | `LinkSys` (type=redis/webhook/credentialSource) | Reference by `namespace/name` in plugin configuration | Distributed rate limiting, Webhook key resolution, CredentialInjector |

For the internal implementation and Handler flow, see [01-architecture/05-resources/18-link-sys.md](../../01-architecture/05-resources/18-link-sys.md).
