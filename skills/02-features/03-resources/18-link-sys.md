---
name: link-sys-detail
description: Full LinkSys CRD schema: Redis/Etcd/Elasticsearch/Webhook/Kafka/OTLP/HttpDns connectors and Controller-managed credential sources with a unified secretRef/TLS credential model.
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
  type: redis                 # redis | etcd | elasticsearch | webhook | kafka | otlp | httpdns | credentialSource
  config:                     # Backend-type-specific configuration (expanded by type below)
    ...
```

`type` and `config` form a tagged enum with `#[serde(tag = "type", content = "config")]`: `type: redis` in YAML corresponds to `RedisClientConfig`, `type: webhook` to `WebhookServiceConfig`, and so on. The older form that placed `type` inside `spec.config.type` is no longer supported.

The CRD accepts any string for `spec.type`; the running Edgion version determines
which types it supports. An unsupported type reports `Accepted=False/Invalid`,
withdraws the previous connector at that key, and cannot be used as a connector.
Correct the type or upgrade the binaries. Unknown payloads are discarded in typed
Controller/Gateway views. Known configs, credentials, and nested enums keep their
validation. Unknown non-credential fields within a supported type are a separate
advisory diagnostic.

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
    db: 0                         # 0-255; the server must expose the database
    auth:                         # Optional; secretRef is required when present
      secretRef:
        name: redis-auth
    pool:
      size: 16
    timeout:
      connect: 3s
      command: 2s
    topology:                     # standalone (default) | sentinel | cluster
      mode: sentinel
      sentinel:
        masterName: mymaster
        sentinels: ["sentinel-1:26379", "sentinel-2:26379"]
      # cluster:
      #   maxRedirects: 6
    tls:
      enabled: true
      clientCertificateRef: { name: redis-client-tls }
```

- When `topology.mode=sentinel`, `sentinel` is required; otherwise the resource is `Accepted=False/Invalid`. Cluster mode uses Edgion defaults when its optional `cluster` block is absent.
- Endpoint forms are `host`, `host:port`, `[IPv6]`, `[IPv6]:port`, `redis://host[:port]`, and `rediss://host[:port]`; the default port is 6379. User information, non-root paths, queries, fragments, unknown schemes, and port 0 are rejected without echoing the submitted value in status or logs.
- Standalone mode requires exactly one top-level `endpoints` item. Cluster mode requires one or more top-level items. Sentinel mode requires top-level `endpoints` to be absent/empty and uses one or more `topology.sentinel.sentinels` items instead. Inactive topology subobjects are rejected.
- Every active endpoint is parsed all-or-nothing. Without `tls.enabled=true`, the active list cannot mix `rediss://` with plaintext `redis://`/scheme-less entries. Explicit TLS applies to the complete redis-rs topology and permits mixed spelling.
- `pool.size` is the target standalone/Sentinel connection count in the range 1-64 (default 8); Cluster ignores it.
- `timeout.connect` and `timeout.command` use GEP-2257 durations. Their defaults are `5s`; the maxima are `10s` and `30s`, respectively.
- Cluster `maxRedirects` defaults to 5 and accepts 0-64. The `cluster` block is optional when the default is sufficient.
- Redis retries connection establishment with fixed bounded exponential backoff, but does not transparently replay an ordinary command after an ambiguous connection failure.
- Redis logging and metrics are owned by the Gateway binary; Redis has no per-connector observability block or arbitrary metric labels.
- `auth` uses `BasicAuthResolver`: the Secret must provide at least `password`; `username` is optional (Redis 6+ ACL).
- redis-rs resolves every configured, reconnect, Sentinel-discovered, and Cluster-discovered dial through the canonical Gateway `EgressResolver` with `ResolverSpec::System`. This happens per connection attempt, not per Redis command. Loopback/link-local/unspecified addresses are rejected in production; private/unique-local addresses follow the Gateway-wide `outbound_egress_block_private` setting. Redis has no resource-level override, and answers live in the shared egress DNS answer cache with `outbound_egress_dns_cache_ttl_ms` as their fallback freshness (classification runs on every use).

### `type: etcd`


Uses the upstream Etcd SDK with static endpoints. Custom message limits,
member discovery, User-Agent, version admission and observability options
are outside the supported configuration.

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
    keepAlive:                    # HTTP/2 keepalive, in seconds
      time: 30
      timeout: 10
      permitWithoutStream: true
    namespace: "/my-app/"         # etcd v3 key prefix
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
      maxResponseBytes: 1048576   # 1 MiB, explicit values clamped to 1..8388608 bytes
      channelSize: 10000          # producer→bulk-loop mpsc capacity, ceiling 100000
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    compatibility:
      vendor: elasticsearch       # elasticsearch | opensearch
```

- Auth tagged-enum: `type: basic` → `BasicAuthResolver`, `type: apiKey` → `EsApiKeyResolver`, `type: bearer` → `BearerResolver`.
- At runtime, requests are issued via reqwest directly over HTTP, without depending on the official `elasticsearch` crate; OpenSearch-compatible.
- `index.datePattern` is a UTC strftime output pattern (default `%Y.%m.%d`).
  Controller reports `Accepted=False/Invalid` and Gateway rejects candidate
  construction when the pattern cannot format, including `%`, trailing `%`,
  unsupported directives and parsing-only `%#z`. Empty patterns, literals and
  `%%` remain valid. This checks format capability, not Elasticsearch index-name
  restrictions. An unexpected runtime formatting failure drops the current batch
  without an ES request/retry and keeps the worker running; transport health and
  first-healthy fallback are unchanged.
- `bulk.maxResponseBytes` bounds acknowledgement reception independently of the
  outgoing request. It defaults to 1 MiB; explicit values are clamped to 1 byte
  through 8 MiB, so zero never disables protection. Increase it for large batches
  or verbose error/metadata envelopes. Declared lengths above the limit are
  rejected early; streamed bytes enforce the limit even without a reliable
  length. Oversized, unreadable or malformed HTTP-success receipts are ambiguous:
  they are not replayed, and the existing healthy/retirement policy remains.
  Their rejected items may consequently lose the opportunity for a 429 retry.
  This is an input limit, not a total exchange heap or process memory guarantee.
- `bulk.maxRetries` is one shared per-sub-batch budget for transport failures, HTTP 5xx responses,
  and item-level 429 rejections. A parsed partial response retries only its 429 documents, in
  request order; later documents wait up to the bounded `bulk.channelSize` capacity. Entries
  submitted after that queue fills are dropped and counted rather than sent to the fallback.
  Exhausting the retry budget marks the connector unhealthy so a configured access-log fallback
  can accept new entries.
- There is no Elasticsearch `failedCache` field. Access-log outage retention is
  configured on the Gateway's `LogConfig`, using an Elasticsearch `output` and
  a bounded LocalFile `fallback`. After recovery, an operator may trigger the
  Elasticsearch-only, at-least-once `POST /api/v1/access-log/backfill` flow.

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
- **`request.method`** (`MethodSpec`): `{ template: String }`. Omit → reuse the frozen incoming method; missing entry capture fails the call without a live-method fallback. An explicit `${method}` template observes the current method, including trusted rewrites. For entry GET rewritten to POST, omitted method sends GET while `${method}` sends POST.
- **`request.{args,headers,cookies}`** (`ListFields`): `{ origin: [OriginField], custom: [CustomField] }`, both default empty. An allow-list — only named fields appear on the wire. Cookies are emitted as a combined `Cookie` header.
- **`OriginField`**: `{ name: String, presence: PresencePolicy = skipIfMissing }`. Copies `name` from the **frozen** entry-time incoming request.
- **`PresencePolicy`** enum (camelCase): `skipIfMissing` (default — omit, no log), `warnIfMissing` (omit + PluginLog alert + continue), `required` (treat as webhook failure: WebhookKeyGet writes no value / `default`; webhook connector does not read `statusOnError` — see the note below).
- **`CustomField`**: `{ name: String, <source> }` where source is exactly one of (`CustomSource`, externally tagged + flattened on serialization). Parsing rejects both sources, missing sources, duplicate visible source keys, and non-string operands (including null and unquoted YAML numbers/booleans); unrelated unknown keys retain advisory handling:
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
- **`success`** (`SuccessSpec`, replaces the removed `successStatusCodes`): `{ statusCodes: [u16], body: [BodyPredicate] }`, both optional. `statusCodes` omitted → any 2xx; an explicit empty list (`[]`) is rejected at admission rather than silently meaning the same thing. `BodyPredicate`'s operator field (`equals`/`notEquals`/`exists`/`in`) has a custom `Deserialize` that enforces **exactly one** operator key at parse time — two operators (or zero) fails to deserialize, so `{pointer: /code, exists: true, equals: 0}` cannot silently keep only the first-matching key. Explicit `equals: null` and `notEquals: null` are valid operands and count as present operator keys; `exists: null` and `in: null` are rejected because their operands must be a boolean and a list, respectively. Predicates are ANDed with the status check and with each other; matched as strict `serde_json::Value` (no coercion — `equals: 0` ≠ `"0"`). A non-JSON body is a `BodyFail` (not an error) when any `body` predicate is configured. A missing `pointer` is also a `BodyFail` for `equals`/`notEquals`/`in` — you cannot assert an equality/membership claim about a field that isn't there — with `exists: false` as the one operator defined to succeed on a missing pointer. Admission requires `maxResponseBytes >= 4096` whenever `success.body` is non-empty; a body truncated by the cap surfaces as `ResponseTooLarge`, not a failed predicate. Passive health (`healthCheck.passive.failureStatusCodes`) still keys off status only — a `success.body` failure never trips the circuit.
- **`retry`** (`WebhookRetry`) gained `backoffPolicy` (`exponential` default | `fixed`), `maxDelayMs` (default 1000, admission ceiling 10000), `jitter` (default true, 50-100% band applied after the cap), `honorRetryAfter` (default false; only takes effect for a 429/503 already listed in `retryOnStatus`; a `Retry-After` above `maxDelayMs` stops the retry loop and returns the response as-is rather than erroring), and `retryOnBodyFailure` (default false; retries a `success.body` predicate failure). `retryDelayMs` is the **base delay of the backoff curve** (`delay = min(maxDelayMs, retryDelayMs << (attempt-1))` under `exponential`), not a fixed interval. **`retryOnTimeout`'s default is now per-consumer**: webhook `false`, `forward_auth` `true` — a connect failure is always safe to retry either way, so `retryOnConnectError` stays `true` for both; only the timeout default is split (webhook POSTs are typically non-idempotent, forward_auth checks are idempotent).
- **`timeoutMsTemplate`**: optional per-request template on the same `${type:name}` engine as `request.*`, but restricted to a **much narrower variable allow-list**: only `${ctx:...}`, `${remoteIp}`, `${matchedHttpRoute:...}`, `${matchedGrpcRoute:...}`. `${header:...}`/`${query:...}`/`${cookie:...}`/`${path}`/`${uri}`/`${requestPath}`/`${requestUri}`/`${method}`/`${secretRef:...}` are all rejected at admission specifically for this field (they remain fully allowed in `request.*`) — it is security-sensitive (call deadline), so it may not be driven by anything the client controls on the request. It is compiled and validated at admission (a malformed template, or one using a disallowed variable type, is `Accepted=False`). `timeoutMsTemplate` parses as `u64` ms, falls back to `timeoutMs` on a non-numeric/empty/below-floor render (floor: `MIN_OUTBOUND_TIMEOUT_MS` = 50ms), and is clamped to 60000ms on the upper end regardless of the render. Every silent fallback increments `edgion_outbound_template_fallback_total{field="timeout_ms"}` (data-plane observability goes through Prometheus, not `tracing`). A `success.body` predicate that fails a 2xx-status response increments `edgion_outbound_success_predicate_fail_total`. **Note:** `statusOnError` is part of the shared `ExternalHttpConnection` schema but is honored by `forward_auth` only — WebhookKeyGet has no fail-close/fail-open decision to make and simply writes no value (or `default`) on any failure; this field is inert when set on a webhook `LinkSys`.
- **Validation** (`Accepted=False` on violation): missing `port`; unsupported `kind`; template parse error, or a client-controlled variable type, in `timeoutMsTemplate` (`request.*` templates keep the full variable set); same name in both `origin` and `custom`; duplicate name within a block; `headers.origin: [{name: Cookie}]` together with a non-empty `cookies` block; `request.body` type `json` whose canary-rendered template does not parse as valid JSON; an explicit `Content-Type` header combined with `request.body` type `json`/`form`; `retry.retryDelayMs == 0`; `retry.maxDelayMs` outside `1..=10000`; `retry.maxRetries` outside `0..=10`; `timeoutMs == 0`; `maxResponseBytes == 0`; `success.statusCodes == []` (explicit empty list); `success.body` non-empty with `maxResponseBytes < 4096`; a `success.body` predicate with zero or more than one operator key (rejected at deserialize time, before this validation function even runs).
- **`statusOnError`** ∈ [200, 599] (validated, but inert for webhook — see above); `maxResponseBytes` clamped to `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes`.

**Consumer side:** the outbound request shape is owned entirely by the webhook resource. The operator-facing consumer is the `WebhookKeyGet` plugin (`WebhookVar`: `webhookRef`, optional `pathOverride`, `extract`, `default`). It passes only the `session` and an optional `pathOverride` (honored only when `path.allowOverride: true`); the `forwardHeaders` / `pathSuffix` knobs are gone. Per-call dynamic values flow in via a preceding CtxSet entry → `${ctx:NAME}` in the webhook templates.

**Removed fields:** `auth`, `defaultHeaders`, and `requestMethod` are ignored at
runtime and reported by the advisory unknown-field detector. Use `request.*`
instead. `successStatusCodes` follows the same unknown-field feedback path;
see the note at the top of this section.

### `type: kafka`

```yaml
spec:
  type: kafka
  config:
    brokers:
      - "kafka:9092"
    channelSize: 4096
    maxTopics: 64
    maxPendingRecords: 4096
    maxPendingBytes: 16777216
    lingerMs: 5
```

- Kafka has a live Gateway provider, runtime store, and full/partial update lifecycle.
- `lingerMs` defaults to 5 and accepts 0 through 29999 milliseconds. Controller
  admission and Gateway construction both reject values at or above the shared
  30-second produce timeout; zero disables the batching delay.
- `channelSize` bounds each topic queue. `maxTopics` (default 64),
  `maxPendingRecords` (default 4096), and `maxPendingBytes` (default 16 MiB)
  bound one client's aggregate queued and in-flight producer state. All limits
  must be greater than zero. Aggregate saturation rejects the new non-blocking
  submission without changing the client's health state.
- `maxPendingBytes` counts UTF-8 payload bytes. It does not include allocator,
  channel-node, task, or Kafka SDK overhead.

### `type: httpdns`

An HTTP DNS resolver: resolves external domains via an HTTP(S) endpoint instead of standard DNS. The `HttpDnsResolverConfig` schema and the shared resolver core live in `edgion-common/src/dns/`. The gateway data-plane wiring is live: a native HTTP/gRPC `Service` reference to an actual `spec.type: ExternalName` Service whose effective resolver (Service → Gateway → EGC precedence) points at an httpdns LinkSys is resolved through this config, cached in the Gateway upstream DNS answer cache, and SSRF-validated. See [`docs/superpowers/specs/2026-07-02-external-dns-resolution-design.md`](../../../docs/superpowers/specs/2026-07-02-external-dns-resolution-design.md) for the full design.

Key fields:
- `preset` (`aliyun` | `tencent`) — selects the response **format** (Aliyun JSON `ips`; Tencent DNSPod `;`-delimited) plus a best-effort built-in URL. The built-in URLs are plain HTTP against the vendors' public resolving IPs and are account-less, so for real vendor accounts combine `preset` with an explicit `urlTemplate`.
- `urlTemplate` — request URL with a `{domain}` placeholder. **Wins over a preset's built-in URL**, so `preset` + `urlTemplate` = the preset's response format with a custom (account-scoped / correct-scheme) endpoint. Required only when no `preset` is set.
- `response` — parsing rules (`json` with `ipPath` and optional `ttlPath`, or `delimited` with `delimiter`); ignored when a `preset` is set. `ttlPath` feeds the answer TTL to the shared DNS answer cache (the `aliyun` preset reads `ttl`).
- `connection` — outbound TLS, total/connect timeouts, native HTTP connection pool,
  bounded response size, and per-resolver loopback exception (no `target`). See the
  [operator connection reference](../../../docs/en/ops-guide/infrastructure/connectors.md#http-dns-resolver)
  for defaults and enforced bounds.
- `fallback` — `system` | `dns` (server list) | `none`, applied on preparation,
  fetch, or parse failure. Gateway publishes the latest prepared or fallback-only
  generation at configuration time, rather than constructing a client per query.

Endpoint hostnames bootstrap through System DNS with the shared EgressResolver;
IP literals are classified before sending. RFC1918/ULA remain allowed;
link-local and unspecified addresses remain blocked even with allowLoopback.
The explicit integration-testing mode keeps the shared loopback exception.
See [operator configuration](../../../docs/en/ops-guide/infrastructure/connectors.md#http-dns-resolver).

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
```

**`publication`:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `persist` | boolean | `true` | `true` writes a generated Secret named after the consuming `EdgionPlugins` (Kubernetes-only); outside Kubernetes it is `Accepted=False` / `UnsupportedConfigurationCenter`. `false` keeps one active token per LinkSys identity (`namespace/name/uid`) in Controller-local memory: no generated Secret, allowed on leader-only Kubernetes and FS/etcd. Kubernetes `ha_mode: all-serve` + `persist: false` is `Accepted=False` / `UnsupportedHaMode`. |

Generated Secrets use the consuming EdgionPlugins namespace and name, independently
of the LinkSys source namespace. The current consumer/source binding and acquired
Secret metadata must match before a generated token is embedded. Local generated
Secret lookup remains qualified by namespace; it does not add a bare `default`
lookup alias. Publication ownership and token validity remain separate checks.
Managed output API reads and write results also require the exact name and explicit
namespace before ownership or concurrency metadata is reused. Creation remains
POST with controlled 409 adoption, and updates use fenced JSONPatch. Cleanup of
old publications does not require the original live source, but retains exact
managed ownership, post-read authorization, and UID/resourceVersion preconditions.

The active-entry map is **Controller-only**. It is never a LinkSys spec field, never on
LinkSys conf_sync, never on LinkSys status, and never on Admin LinkSys
`/configclient`. The active token is still copied onto the consuming
EdgionPlugins `resolvedCredential` (Admin redacts that value).

Rotation replaces the active entry atomically; this map provides no previous-token
fallback. Existing Arc holders and Gateway generations keep their own lifetimes.
The testing-mode store statistics count one `credential_memory_keys` entry per
stored source identity; retired entries held by other owners are not counted.
Proactive acquisition, publication retries, and bootstrap `previousSecretRef`
fallback remain separate mechanisms; see the
[credential lifecycle](../../../docs/en/user-guide/http-route/filters/edgion-plugins/credential-injector.md#credential-lifecycle).

## Status Schema

In Kubernetes, this native payload is nested under each
`status.controllers[].status`. Select the entry only by `controllerName`.
All lifecycle fields (including credentialState, generations, refresh timestamps and consumer
counts) belong to that same entry. There is no top-level aggregate conditions or
lifecycle status. FileSystem/etcd retain the native shape shown
below. conf_sync carries complete source status plus native `spec.currentStatus`.
The shared envelope has a 32-entry cap; see
[status ownership](../../01-architecture/01-controller/10-status-management.md#7-status-persistence).

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
- `False/Invalid` — has validation errors (invalid Redis endpoint syntax/cardinality/transport, missing Sentinel config, empty Etcd/Elasticsearch endpoints, invalid Webhook URI, etc.); `message` concatenates the errors.
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
- Present as `True/UnknownFieldsDetected` when a typed Redis, Etcd, Elasticsearch, Kafka, OTLP,
  Webhook, HttpDns, or credentialSource configuration contains an unrecognized key.
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
- **Gateway side**: after `LinkSysStore` receives the spec, it uses the resolvers in `edgion-resources/src/credentials` to recompute `Material` from the embedded `Secret.data`. Redis, Etcd, Elasticsearch, Kafka, and OTLP use a per-key persistent runner (`get_*_client` stays unset until the first successful publish); Webhook skips preparation on an equal runtime fingerprint and publishes prepared clients in the same unified runtime table. Of the four credential states (`NotRequired` / `Pending` / `Invalid` / `Resolved`), only the former or `Resolved` allows building a client; the rest fail-fast and **do not** degrade to anonymous connections.
- **Hot reload**: when a referenced Secret changes, the Controller reprocesses LinkSys and pushes the resolved spec via conf_sync. An ordinary update with unchanged consumed runtime material reuses the current client. A material change starts a replacement: persistent providers with live connections keep the old generation until a new one initializes and publishes successfully, then retire it; providers without live connections retire the old generation before attempting the new one. Webhook prepares before same-type replacement. Type changes remove the previous provider before creating the new one and may temporarily leave no client. Deletion removes current lookup and signals asynchronous cleanup without waiting for every retained handle. Retirement does not guarantee draining or delivery of queued Kafka/Elasticsearch records.
- **`credentialSource`**: the Controller credential manager acquires and rotates the token. Persist-on writes a generated Secret; persist-off stores the map on `ControllerRuntime.credential_memory`. LinkSys conf_sync never carries that map. The Gateway `LinkSysStore` is a no-op for this variant; injection uses EdgionPlugins `resolvedCredential`.
- **Publication mode changes**: persist-on to persist-off establishes a live memory credential before owned generated Secrets are deleted, and failed deletes are retried. Persist-off to persist-on keeps memory until every current consumer Secret is consistent.
- **Ephemeral teardown**: leadership loss, Controller runtime reset/restart, LinkSys delete or type change, LinkSys UID replacement, last-consumer removal, and wipe-all clear the memory slot. The manager reacquires on the next active reconcile. A Gateway may continue using a generation already embedded in its PluginRuntime until `validUntil`; requests fail closed when no unexpired resolved credential remains.
- **Consumer limit**: one EdgionPlugins resource may contain at most one enabled LinkSys-backed CredentialInjector. Static Secret-backed injectors do not count toward this limit.

## Cross-references and Dependencies

| Reference Direction | Target Resource | Mechanism | Typical Scenario |
|---------------------|-----------------|-----------|------------------|
| LinkSys → Secret | `Secret` | `secretRef` / `tls.certs.secretRef` + `SecretRefManager` | All auth / mTLS credentials |
| LinkSys → ReferenceGrant | `ReferenceGrant` | Authorization for cross-namespace `secretRef` | Centralize credentials in a `credentials` namespace |
| AccessLog → LinkSys | `LinkSys` (type=elasticsearch/kafka/otlp) | Reference by `namespace/name` in configuration | External log output; LocalFile is configured directly and is not LinkSys |
| EdgionPlugins → LinkSys | `LinkSys` (type=redis/webhook/credentialSource) | Reference by `namespace/name` in plugin configuration | Distributed rate limiting, Webhook key resolution, CredentialInjector |

For the internal implementation and Handler flow, see [01-architecture/05-resources/18-link-sys.md](../../01-architecture/05-resources/18-link-sys.md).

## OTLP/gRPC

`type: otlp` selects `OtlpClientConfig`: required `endpoint`, `timeoutMs` default
10000 (1..=300000), optional `auth.secretRef`, optional shared `tls`. Unknown
fields are rejected. `auth` reads Secret `bearerToken` via the canonical Bearer
parser with an OTLP-only 8192-byte cap. There is no protocol selector, arbitrary
metadata map or inline credential. Endpoints accept only HTTP/HTTPS origins with
optional root slash; omitted ports mean 80/443, and collectors normally use :4317.
IPv6 must be bracketed; userinfo, query, fragment, non-root path, escapes and
whitespace are rejected without echoing input.

HTTPS always uses the canonical outbound TLS resolver: public webpki roots or replacement custom
CA, optional mTLS identity, original-host verification. Enabled verify=false,
hostname and SAN overrides are rejected. Disabled local TLS is inert and does not
disable HTTPS. HTTP cannot enable a local TLS policy. All credential/certificate
refs are validated core Secrets with normal ReferenceGrant rules. Resolved
material remains schemars-excluded, admin-redacted and conf-sync-serialized.

StringOutput is `otlp: {linkSysRef: {namespace: observability, name: collector}}`.
Namespace/name are validated on process deserialization before logging, including
all fallback/disabled blocks. Missing/wrong-type/unhealthy references use existing
first-healthy fallback. Connection initialization failure is not process startup
failure; invalid process YAML is. See the complete
[operator example](../../../docs/en/ops-guide/infrastructure/connectors.md#otlpgrpc-connector).

LinkSys auth.secretRef declares all four operator fields (name, namespace, group,
kind) in the structural CRD subtree so Kubernetes cannot prune unsupported group
or kind before typed validation. The focused CRD guard compares this subtree to
SecretObjectReference. Handler apply/delete clear both Secret and cross-namespace
reference managers; failed references still rebuild their current wakeup edges.


For Redis/Etcd auth, Elasticsearch auth, Kafka SASL and OTLP auth, a carried Secret
must match the configured Secret reference's name and explicit namespace before
credentials can be used. Omitted reference scope uses the LinkSys namespace
(default when the owner omits it); a namespace-less raw Secret is not a qualified
default Secret. Missing material remains pending, and mismatched material is
invalid. This does not add fields, Secret UID requirements or authorization rules.
Ordinary same-type invalid updates retain an already-published client under the
existing provider policy; forced replacements, deletion and type changes keep
their existing retirement behavior.


### Redis client runtime contract

Standalone and Sentinel use `pool.size` (1..64, default 8). Cluster uses one
multiplexed topology client and ignores `pool.size`. Operations have one total
`timeout.command` budget, including MOVED and NOSCRIPT fallback. Unknown outcomes
are never replayed; ASK, TRYAGAIN and CLUSTERDOWN fail the operation. Only MOVED
is followed, up to `maxRedirects` (default 5). Multi-key commands must stay in one
Cluster hash slot; cross-slot operations do not fan out.

Health uses active PING/ROLE probes at a nominal five-second cadence. Scheduling
and probe duration add latency; coverage may conservatively exclude unvisited
primaries. System TLS trust uses compiled-in webpki roots. Configure explicit CA
references for private CAs. Sentinel nodes share data-node credentials. Published
Cluster recovery dials the current known primaries together with the configured
seeds. A new generation must establish real connectivity before ordinary
replacement; failed ordinary updates retain the last published generation.
Forced refresh keeps retire-first behavior.

### Address families and complete answers

A custom `urlTemplate` may include `{recordType}`, for example
`https://dns.example/resolve?host={domain}&type={recordType}`. The resolver substitutes
`A` or `AAAA`; `all` performs two sequential requests under one configured primary
`connection.timeoutMs` deadline. A custom template without this token declares one
fixed/combined complete answer: the operator must ensure the provider supports the
requested families. Built-in preset URLs without a custom template support IPv4
primary queries only; IPv6/`all` use the configured fallback or fail.

Family-aware queries reject malformed address values, missing/wrong response
containers, wrong-family typed answers, and more than 256 unique canonical addresses
before filtering (including merged typed answers). An explicitly empty array/string
or empty delimited body is a valid empty typed family; one nonempty family plus one
valid empty family succeeds. A failed family is not authoritative absence, and two
empty families fail. A fallback always retains the requested family. System DNS
cannot reveal partial failures hidden by the OS; combined HTTPDNS likewise relies
on the provider's successful response being complete. HTTPDNS bootstrap keeps its
native system-resolution policy. Changing backend family does not rebuild its
prepared client or duplicate TLS material.
