---
name: link-sys-detail
description: Full LinkSys CRD schema: Redis/Etcd/Elasticsearch/Webhook/Kafka connector configuration with a unified secretRef/TLS credential model.
---

# LinkSys Resource

> API: `edgion.io/v1` | Scope: Namespaced | ShortName: `lsys`

LinkSys is the unified external-system connector resource. The top-level `spec.type` distinguishes the backend, and `spec.config` carries the detailed configuration for that backend. All credentials go through `secretRef` referencing K8s `Secret`; **inlining** sensitive fields such as username, password, token, or `clientKey` in the CRD is **not allowed** (they are hard-rejected at deserialization by `deny_unknown_fields`).

## Top-level Schema

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: my-redis
  namespace: default
spec:
  type: redis                 # redis | etcd | elasticsearch | webhook | kafka
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
| basic (Redis/Etcd/ES/Webhook) | `kubernetes.io/basic-auth` or `Opaque` | `password` | `username` (default = backend default ACL) |
| apiKey (Elasticsearch) | `Opaque` | `apiKeyId`, `apiKeySecret` | — |
| bearer (Elasticsearch/Webhook) | `Opaque` | `bearerToken` | — |
| hmac (Webhook) | `Opaque` | `hmacKey` | — |
| TLS (all providers) | `kubernetes.io/tls` or `Opaque` | **At least one of** `tls.crt` / `tls.key` / `ca.crt` | — |

All application-layer key names use **camelCase**; TLS keys follow the K8s `kubernetes.io/tls` standard (which contains dots).

> **Breaking change notice**: all `*Auth` structs have `deny_unknown_fields` enabled. Writing `username: ...` / `password: ...` / `bearerToken: ...` / `apiKeyId: ...` / `clientKey: ...` directly in the CRD YAML causes parsing to fail (it is not silently ignored).

### TLS Credential Structure

```yaml
tls:
  enabled: true
  insecureSkipVerify: false       # Development/testing only; Redis/Etcd do not support it — setting true rejects startup outright
  certs:
    caCert: |                      # Public material, can be inlined
      -----BEGIN CERTIFICATE-----
      ...
    clientCert: |                  # Public material, can be inlined
      -----BEGIN CERTIFICATE-----
      ...
    secretRef:                     # The private key tls.key must come from secretRef
      name: my-client-tls
      # Defaults to the same namespace as LinkSys when omitted
```

- The client private key **must** be provided via `certs.secretRef`, pointing to a `kubernetes.io/tls` (key `tls.key`) or `Opaque` (same key name) Secret.
- `caCert` / `clientCert` are public material; the inline PEM capability is preserved.
- For providers whose URLs already imply TLS (`https://`, `rediss://`), `enabled: true` only additionally enables custom CA / client certificate handling; `tls` may be omitted when only system CAs are used.

## Configuration by `type`

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
      certs:
        secretRef: { name: redis-client-tls }
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
      certs:
        secretRef: { name: etcd-client-tls }
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
- etcd-client does not support `insecureSkipVerify`: setting it explicitly to `true` directly `bail!`s and rejects startup (consistent with Redis, avoiding the "silently ignored yet still strictly verifies" configuration illusion). To skip verification, use other mechanisms.

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
    failedCache:                  # PLANNED, not yet implemented — pruned by the API server, no effect today
      type: localFile             # localFile | redis
      localFile:
        path: "/var/log/edgion/es-failed"
      # redis:
      #   linkSysRef: "default/redis-cache"
      #   listKey: "es-failed"
      #   maxEntries: 100000
    compatibility:
      vendor: elasticsearch       # elasticsearch | opensearch
```

- Auth tagged-enum: `type: basic` → `BasicAuthResolver`, `type: apiKey` → `EsApiKeyResolver`, `type: bearer` → `BearerResolver`.
- At runtime, requests are issued via reqwest directly over HTTP, without depending on the official `elasticsearch` crate; OpenSearch-compatible.
- `failedCache` (spill-on-outage + replay-on-recovery) is **planned, not yet implemented** — the field shown above is a design placeholder and is pruned by the API server today (it has no effect). See `edgion-gateway/src/link_sys/readme`.

### `type: webhook`

```yaml
spec:
  type: webhook
  config:
    uri: "https://key-service.internal/resolve"   # Required, http/https
    requestMethod: POST                            # Default GET
    timeoutMs: 3000
    auth:                                          # Optional
      type: bearer                                 # basic | bearer | hmac
      secretRef:
        name: webhook-token
    tls:
      enabled: true
      certs:
        secretRef: { name: webhook-client-tls }
    defaultHeaders:                                # Static outgoing headers, sent on every call
      X-Source: edgion
    successStatusCodes: [200]
    maxResponseBytes: 2048                         # Default hard upper bound 16 KiB (configurable via EdgionGatewayConfig.linkSys.webhookMaxResponseBytes)
    allowDegradation: true                         # Silently degrade to None on failure
    statusOnError: 503                             # Status code returned when allowDegradation=false and the webhook is the sole key source
    retry:
      maxRetries: 1
      retryDelayMs: 100
      retryOnTimeout: true
      retryOnConnectError: true
      retryOnStatus: [502, 503, 504]
    rateLimit:                                     # Protection for the external webhook
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
        # backoff:                                 # Effective only when active is disabled
        #   initialSec: 5
        #   multiplier: 2.0
        #   maxSec: 60
```

- Auth tagged-enum: `type: basic` → `BasicAuthResolver`, `type: bearer` → `BearerResolver`, `type: hmac` → `HmacResolver` (Secret key `hmacKey`), `type: apiKey` → `WebhookApiKeyResolver` (Secret key `apiKey`, injected as a header or query parameter).
- `uri` must be `http://` or `https://`; `requestMethod` must be a valid HTTP method; `statusOnError` ∈ [200, 599]; `maxResponseBytes` is clamped at call time to `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes` (default 16 KiB).
- Call semantics (method/path/headers/body) are decided by the consumer: `KeyGet::Webhook` selects which inbound request headers to forward via `forwardHeaders`, and appends `pathSuffix` to the end of `uri`; other consumers customize via `get_webhook_call_handle()` + `WebhookCallRequest`.

### `type: kafka`

```yaml
spec:
  type: kafka
  config:
    brokers:
      - "kafka:9092"
```

- Kafka is currently a placeholder (the schema is valid, but the Gateway side has not yet integrated a provider). `Accepted=True`; runtime availability depends on later releases.

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
```

**`Accepted`**:
- `True/Accepted` — structure + `validate_config()` both passed.
- `False/Invalid` — has validation errors (empty endpoints, missing Sentinel/Cluster sub-config, invalid Webhook URI, etc.); `message` concatenates the errors.

**`ResolvedRefs`**:
- `True/ResolvedRefs` — all referenced `Secret`s have been found, `Secret.type` is valid, and all required keys exist and are decodable.
- `False/SecretNotFound` — the Secret has not yet appeared in the apiserver (the Controller automatically requeues on Secret creation events).
- `False/RefNotPermitted` — cross-namespace, no matching `ReferenceGrant`.
- `False/InvalidSecretType` — `Secret.type` is not in the resolver's allowlist (e.g., basic auth received `kubernetes.io/dockerconfigjson`).
- `False/MissingKeys` — the Secret is missing required keys (e.g., basic missing `password`, apiKey missing `apiKeyId` + `apiKeySecret`).
- `False/DecodeError` — UTF-8 decoding or JSON parsing of a key failed (only ACME private keys go through JSON validation; LinkSys essentially never hits this).

`kubectl get linksys` directly displays the `Accepted` / `ResolvedRefs` columns.

## Runtime Behavior

- **Controller side**: parse `secretRef` → register an owner→secret dependency in `SecretRefManager` (Secret update/delete cascades requeue this resource) → embed the entire `Secret` into `spec.config.<auth>.secret` / `spec.config.tls.certs.secret` → sync to the Gateway via conf_sync gRPC. The `Secret` itself is a `no_sync_kind` and is not synced separately.
- **Gateway side**: after `LinkSysStore` receives the spec, it uses the resolvers in `edgion-resources/src/credentials` to recompute `Material` from the embedded `Secret.data` and hands it to the Redis / Etcd / ES / Webhook client to build the connection. Of the four credential states (`NotRequired` / `Pending` / `Invalid` / `Resolved`), only the former or `Resolved` allows building a client; the rest fail-fast and **do not** degrade to anonymous connections.
- **Hot reload**: when a Secret pointed to by secretRef changes → the Controller recomputes → pushes the new spec via conf_sync → the Gateway rebuilds the corresponding provider client and the old connections drain gracefully.

## Cross-references and Dependencies

| Reference Direction | Target Resource | Mechanism | Typical Scenario |
|---------------------|-----------------|-----------|------------------|
| LinkSys → Secret | `Secret` | `secretRef` / `tls.certs.secretRef` + `SecretRefManager` | All auth / mTLS credentials |
| LinkSys → ReferenceGrant | `ReferenceGrant` | Authorization for cross-namespace `secretRef` | Centralize credentials in a `credentials` namespace |
| AccessLog → LinkSys | `LinkSys` (type=elasticsearch/redis/webhook) | Reference by `namespace/name` in configuration | Log output to ES / Redis |
| EdgionPlugins → LinkSys | `LinkSys` (type=redis/webhook) | Reference by `namespace/name` in plugin configuration | Distributed rate limiting, Webhook key resolution |

For the internal implementation and Handler flow, see [01-architecture/05-resources/18-link-sys.md](../../01-architecture/05-resources/18-link-sys.md).
