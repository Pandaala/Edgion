# Connectors (LinkSys)

> **🔌 Edgion extension**
> 
> Connectors are external-system integrations defined by the `LinkSys` CRD (`edgion.io/v1`); they are not part of standard Gateway API.

## Overview

A **connector** is a reusable, named handle to an external system — Redis, Etcd, Elasticsearch, an HTTP webhook, or Kafka. You declare it once as a `LinkSys` resource, attach its credentials through Kubernetes Secrets, and then reference it from any feature that needs that backend. This keeps connection details and credentials in one place instead of duplicating them across every plugin and policy.

A single `LinkSys` object describes exactly one connector. The top-level `spec.type` selects the backend, and `spec.config` carries the configuration for that backend. Several Edgion features consume connectors by `namespace/name`:

- Distributed (cluster-wide) rate limiting references a `redis` connector by `namespace/name` — see [Redis-backed Rate Limit](../../user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md).
- Access and audit log sinks reference an `elasticsearch`, `redis`, or `webhook` connector — see [Access Log](../observability/access-log.md).
- Plugins that resolve keys or external data (for example webhook-based key resolution) reference a `webhook` connector.

`LinkSys` is a Namespaced resource with the short name `lsys`. `kubectl get linksys` shows the `Accepted` and `ResolvedRefs` conditions at a glance.

```text
                 reference by namespace/name
  ┌──────────────┐        ┌──────────────────┐        ┌────────────────┐
  │ EdgionPlugins│───────▶│  LinkSys (lsys)  │───────▶│ External system│
  │ AccessLog    │        │  type + config   │        │ Redis / Etcd / │
  │ ...          │        │  + secretRef     │        │ ES / Webhook   │
  └──────────────┘        └────────┬─────────┘        └────────────────┘
                                   │ secretRef
                                   ▼
                          ┌──────────────────┐
                          │ Secret (creds /  │
                          │ TLS private key) │
                          └──────────────────┘
```

⚠️ **All credentials (passwords, tokens, API keys, HMAC keys, client private keys) must come from Secret references. Inlining sensitive fields in the CRD is rejected at parse time** — the auth and TLS structs enable `deny_unknown_fields`, so writing `username`, `password`, `bearerToken`, `apiKeyId`, `hmacKey`, or `clientKey` directly in YAML fails validation rather than being silently ignored.

## Credential model

All connectors share the same credential conventions.

### Secret references

Credentials are referenced with a `secretRef` object:

```yaml
secretRef:
  name: redis-auth           # Required
  namespace: credentials     # Optional; defaults to the LinkSys namespace
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Name of the Secret to read credentials from. |
| `namespace` | String | LinkSys namespace | Namespace of the Secret. Cross-namespace references require a `ReferenceGrant`. |
| `group` | String | `""` (core) | API group of the referenced object. Reserved for future non-Secret sources; leave unset. |
| `kind` | String | `Secret` | Kind of the referenced object. Reserved for future non-Secret sources; leave unset. |

Cross-namespace references (where `namespace` differs from the LinkSys resource's own namespace) require a Gateway API `ReferenceGrant` permitting `Secret` in the target namespace. Without it, the status reports `ResolvedRefs=False/RefNotPermitted`. See [Cross-Namespace Reference](./reference-grant.md).

### Secret key convention

Secret keys follow a fixed, well-known convention — there are no `usernameKey` / `passwordKey` fields to customize.

| Auth type | Recommended `Secret.type` | Required keys | Optional keys |
|-----------|---------------------------|---------------|---------------|
| basic (Redis / Etcd / ES / Webhook) | `kubernetes.io/basic-auth` or `Opaque` | `password` | `username` |
| apiKey (Elasticsearch) | `Opaque` | `apiKeyId`, `apiKeySecret` | — |
| bearer (Elasticsearch / Webhook) | `Opaque` | `bearerToken` | — |
| hmac (Webhook) | `Opaque` | `hmacKey` | — |
| apiKey (Webhook) | `Opaque` | `apiKey` | — |
| TLS (all backends) | `kubernetes.io/tls` or `Opaque` | At least one of `tls.crt` / `tls.key` / `ca.crt` | — |

Application-layer key names use camelCase; TLS keys follow the Kubernetes `kubernetes.io/tls` standard.

### TLS

Every backend accepts the same `tls` block for securing the connection.

```yaml
tls:
  enabled: true
  insecureSkipVerify: false
  certs:
    # CA and client certificates are public material and may be inlined.
    caCert: |
      -----BEGIN CERTIFICATE-----
      <base64-encoded-cert>
      -----END CERTIFICATE-----
    clientCert: |
      -----BEGIN CERTIFICATE-----
      <base64-encoded-cert>
      -----END CERTIFICATE-----
    # The client private key (tls.key) must come from a Secret.
    secretRef:
      name: my-client-tls
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | Boolean | `false` | Enable TLS for the connection. For URLs that already imply TLS (`https://`, `rediss://`), this only adds custom CA / client-certificate handling. |
| `insecureSkipVerify` | Boolean | `false` | Skip certificate verification. ⚠️ **Development and testing only.** Redis and Etcd do not support it — setting `true` rejects startup outright. |
| `certs.caCert` | String | none | CA certificate PEM (public material; inlineable). |
| `certs.clientCert` | String | none | Client certificate PEM for mTLS (public material; inlineable). |
| `certs.secretRef` | Object | none | Secret reference supplying the client private key `tls.key`. **The private key must come from a Secret, never inline.** |

⚠️ **The client private key must always be supplied through `certs.secretRef`.** Only the CA and client certificate (public material) may be inlined as PEM.

### Observability

Redis, Etcd, and Elasticsearch accept a shared `observability` block.

```yaml
observability:
  logging:
    enabled: true
    level: info
    slowLogThreshold: 200
  metrics:
    enabled: true
    labels:
      env: production
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `logging.enabled` | Boolean | `false` | Enable per-connector logging. |
| `logging.level` | String | none | Log level. Example: `info`, `debug`, `warn`, `error`. |
| `logging.slowLogThreshold` | Integer | none | Slow-operation log threshold in milliseconds. |
| `metrics.enabled` | Boolean | `false` | Enable per-connector metrics. |
| `metrics.labels` | Object | none | Extra key/value labels attached to metrics. |

## Redis connector

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: cache-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "redis://redis.production.svc:6379"
    db: 0
    auth:
      secretRef:
        name: redis-auth
    pool:
      size: 16
      minIdle: 2
    timeout:
      connect: 3000
      read: 2000
      write: 2000
```

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoints` | Array\<String\> | **Required** | Redis endpoints; at least one. Example: `"redis://redis:6379"` or `"rediss://..."` for TLS. |
| `db` | Integer | none | Database index (0–15). |
| `auth` | Object | none | Authentication. When present, `auth.secretRef` is required. The Secret must provide `password`; `username` is optional (Redis 6+ ACL). |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). |
| `observability` | Object | none | Logging and metrics; see [Observability](#observability). |

### Connection pool and timeouts

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `pool.size` | Integer | none | Maximum pool size. |
| `pool.minIdle` | Integer | none | Minimum idle connections to keep. |
| `timeout.connect` | Integer | none | Connection timeout in milliseconds. |
| `timeout.read` | Integer | none | Read timeout in milliseconds. |
| `timeout.write` | Integer | none | Write timeout in milliseconds. |
| `retry.maxRetries` | Integer | none | Maximum operation retries. |
| `retry.backoff.type` | String | **Required** (when `backoff` is set) | Backoff strategy. Example: `exponential`, `linear`, `fixed`. |
| `retry.backoff.initialDelay` | Integer | none | Initial retry delay in milliseconds. |
| `retry.backoff.maxDelay` | Integer | none | Maximum retry delay in milliseconds. |
| `retry.backoff.multiplier` | Number | none | Multiplier for exponential backoff. |

### Topology: standalone, sentinel, cluster

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `topology.mode` | Enum | — (required when `topology` is set) | Deployment topology. Allowed values: `standalone` \| `sentinel` \| `cluster`. |
| `topology.sentinel` | Object | none | Sentinel configuration. **Required when `mode=sentinel`.** |
| `topology.cluster` | Object | none | Cluster configuration. **Required when `mode=cluster`.** |
| `topology.sentinel.masterName` | String | **Required** | Master name monitored by Sentinel. |
| `topology.sentinel.sentinels` | Array\<String\> | **Required** | Sentinel addresses (`host:port`). |
| `topology.sentinel.password` | String | none | Sentinel's own password, independent of Redis node auth. |
| `topology.cluster.readFromReplicas` | Boolean | none | Allow reads to be served from replica nodes. |
| `topology.cluster.maxRedirects` | Integer | none | Maximum cluster redirects to follow. |

When `topology` is omitted, the connector runs in standalone mode. When `mode=sentinel` but `sentinel` is missing (or `mode=cluster` but `cluster` is missing), the status reports `Accepted=False/Invalid`.

#### Example: Sentinel high availability

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: ha-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "redis://sentinel-1:26379"
    auth:
      secretRef:
        name: redis-auth
    topology:
      mode: sentinel
      sentinel:
        masterName: mymaster
        sentinels:
          - "sentinel-1:26379"
          - "sentinel-2:26379"
          - "sentinel-3:26379"
```

#### Example: Cluster mode

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: cluster-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "redis://redis-node-1:6379"
      - "redis://redis-node-2:6379"
      - "redis://redis-node-3:6379"
    auth:
      secretRef:
        name: redis-auth
    topology:
      mode: cluster
      cluster:
        readFromReplicas: true
        maxRedirects: 6
```

## Etcd connector

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: config-etcd
  namespace: production
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1.production.svc:2379"
      - "https://etcd-2.production.svc:2379"
    auth:
      secretRef:
        name: etcd-creds
        namespace: credentials
    tls:
      enabled: true
      certs:
        secretRef:
          name: etcd-client-tls
    namespace: "/my-app/"
```

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoints` | Array\<String\> | **Required** | Etcd endpoints; at least one. Example: `"https://etcd-1:2379"`. |
| `auth` | Object | none | Authentication. The Secret typically provides both `username` and `password`. |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). Etcd does **not** support `insecureSkipVerify` — setting `true` rejects startup. |
| `namespace` | String | none | Etcd v3 key prefix. |
| `userAgent` | String | none | HTTP User-Agent sent to etcd. Example: `"edgion-gateway/v1"`. |
| `rejectOldCluster` | Boolean | none | Reject connecting to an outdated cluster. |
| `observability` | Object | none | Logging and metrics; see [Observability](#observability). |

### Timeouts and keepalive

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timeout.dial` | Integer | none | Dial timeout in milliseconds. |
| `timeout.request` | Integer | none | Request timeout in milliseconds. |
| `timeout.keepAlive` | Integer | none | Keepalive timeout in milliseconds. |
| `keepAlive.time` | Integer | none | HTTP/2 keepalive time in seconds. |
| `keepAlive.timeout` | Integer | none | HTTP/2 keepalive timeout in seconds. |
| `keepAlive.permitWithoutStream` | Boolean | none | Allow keepalive pings without active streams. |
| `autoSyncInterval` | Integer | none | Endpoint auto-sync interval in seconds; `0` disables it. |
| `maxCallSendSize` | Integer | none | Maximum send message size in bytes. |
| `maxCallRecvSize` | Integer | none | Maximum receive message size in bytes. |

## Elasticsearch connector

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: logs-es
  namespace: production
spec:
  type: elasticsearch
  config:
    endpoints:
      - "https://es-1.production.svc:9200"
      - "https://es-2.production.svc:9200"
    auth:
      type: apiKey
      secretRef:
        name: es-api-key
    tls:
      enabled: true
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    bulk:
      batchSize: 500
      flushInterval: 5000
```

Multiple endpoints are used for round-robin load balancing. This connector is also OpenSearch-compatible; requests are issued directly over HTTP.

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoints` | Array\<String\> | **Required** | Elasticsearch endpoints; at least one. Example: `"https://es-1:9200"`. |
| `auth` | Object | none | Authentication (tagged enum). `type` is required when `auth` is present. |
| `auth.type` | Enum | **Required** (when `auth` is set) | Auth method. Allowed values: `basic` \| `apiKey` \| `bearer`. |
| `auth.secretRef` | Object | **Required** (when `auth` is set) | Secret supplying the credentials for the chosen `auth.type`. |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). |
| `compatibility.vendor` | Enum | `elasticsearch` | Target vendor. Allowed values: `elasticsearch` \| `opensearch`. |
| `observability` | Object | none | Logging and metrics; see [Observability](#observability). |

### Connection pool and timeouts

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timeout.connect` | Integer | none | Connection timeout in milliseconds. |
| `timeout.request` | Integer | none | Request timeout in milliseconds. |
| `pool.maxIdlePerHost` | Integer | none | Maximum idle connections per host. |
| `pool.idleTimeout` | Integer | none | Idle connection timeout in milliseconds. |

### Index naming

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `index.prefix` | String | `"edgion-logs"` | Index name prefix. |
| `index.datePattern` | String | `"%Y.%m.%d"` | strftime date pattern appended to the index name. Produces indices like `edgion-logs-2026.05.29`. |

### Bulk loader

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bulk.batchSize` | Integer | `500` | Documents per bulk batch. |
| `bulk.flushInterval` | Integer | `5000` | Flush interval in milliseconds, even if the batch is not full. |
| `bulk.maxRetries` | Integer | `3` | Maximum retries on bulk failure. |
| `bulk.backoffMs` | Integer | `1000` | Initial backoff between retries in milliseconds. |
| `bulk.maxBodyBytes` | Integer | `10485760` (10 MB) | Maximum bulk request body size in bytes. Ceiling 50 MB. |
| `bulk.channelSize` | Integer | `10000` | Producer-to-bulk-loop channel capacity. Ceiling 100000. |

### Failed-document cache

When indexing fails, documents can be buffered for later retry.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `failedCache.type` | Enum | **Required** (when `failedCache` is set) | Cache backend. Allowed values: `localFile` \| `redis`. |
| `failedCache.localFile.path` | String | **Required** (for `localFile`) | Directory path for failed-document storage. |
| `failedCache.redis.linkSysRef` | String | **Required** (for `redis`) | `namespace/name` of a Redis connector. |
| `failedCache.redis.listKey` | String | none | Redis list key for failed documents. |
| `failedCache.redis.maxEntries` | Integer | none | Maximum entries retained in the list. |

#### Example: Elasticsearch with Redis-backed failed cache

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: logs-es
  namespace: production
spec:
  type: elasticsearch
  config:
    endpoints:
      - "https://es-1.production.svc:9200"
    auth:
      type: apiKey
      secretRef:
        name: es-api-key
    tls:
      enabled: true
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    failedCache:
      type: redis
      redis:
        linkSysRef: "production/cache-redis"
        listKey: "es-failed"
        maxEntries: 100000
```

## Webhook connector

The webhook connector requires a `target`, which is a tagged choice between an external URI (`type: uri`) and a Kubernetes Service (`type: service`).

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      type: uri
      uri: "https://key-service.internal/resolve"
    requestMethod: POST
    timeoutMs: 3000
    auth:
      type: bearer
      secretRef:
        name: webhook-token
    defaultHeaders:
      X-Source: edgion
    successStatusCodes: [200]
    maxResponseBytes: 2048
```

### Target

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target.type` | Enum | **Required** | Target kind. Allowed values: `uri` \| `service`. |
| `target.uri` | String | **Required** (for `uri`) | External endpoint; must start with `http://` or `https://`. |
| `target.name` | String | **Required** (for `service`) | Kubernetes Service name. |
| `target.namespace` | String | LinkSys namespace | Service namespace. |
| `target.port` | Integer | none | Service port; must be greater than 0 when set. |
| `target.kind` | String | `Service` | Service kind; for example `Service` or `ServiceClusterIp`. |

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `requestMethod` | String | `GET` | HTTP request method; must be a valid HTTP method. |
| `timeoutMs` | Integer | `5000` | Request timeout in milliseconds; must be greater than 0. |
| `auth` | Object | none | Authentication (tagged enum); see below. |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). Defaults to the system CA bundle when unset. |
| `defaultHeaders` | Object | none | Static headers sent on every call (per-call headers win on conflict). |
| `successStatusCodes` | Array\<Integer\> | any 2xx | HTTP status codes treated as success. |
| `maxResponseBytes` | Integer | `1024` | Maximum response body size to read in bytes. Clamped at call time to `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes` (default 16 KiB). |

### Authentication

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `auth.type` | Enum | **Required** (when `auth` is set) | Auth method. Allowed values: `basic` \| `bearer` \| `hmac` \| `apiKey`. |
| `auth.secretRef` | Object | **Required** (when `auth` is set) | Secret supplying the credential for the chosen `auth.type`. |
| `auth.in` | String | **Required** (for `apiKey`) | Where to inject the key. Allowed values: `header` \| `query`. |
| `auth.headerName` | String | none | Header name when `in=header`. Example: `X-API-Key`. |
| `auth.queryParam` | String | none | Query parameter name when `in=query`. Example: `api_key`. |

### Degradation and error handling

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allowDegradation` | Boolean | `false` | When `true`, a webhook failure is silently skipped and the consumer's fallback chain continues. When `false`, failure causes the lookup to fail. |
| `statusOnError` | Integer | `503` | Status code returned when `allowDegradation=false` and the webhook is the sole source. Must be in `[200, 599]`. |

### Retry

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `retry.maxRetries` | Integer | `1` | Maximum retries (not counting the initial call). |
| `retry.retryDelayMs` | Integer | `100` | Delay between retries in milliseconds. |
| `retry.retryOnTimeout` | Boolean | `true` | Retry on request timeout. |
| `retry.retryOnConnectError` | Boolean | `true` | Retry on connection error. |
| `retry.retryOnStatus` | Array\<Integer\> | `[]` | HTTP status codes that trigger a retry. |

When `retry` is omitted, the webhook fails immediately on the first error.

### Rate limit

Protects the external webhook service from being overwhelmed by the gateway.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rateLimit.rate` | Integer | **Required** (when `rateLimit` is set) | Maximum calls allowed per window. |
| `rateLimit.windowSec` | Integer | `1` | Window size in seconds. |

### Health check

Supports active probing, passive monitoring, or both (recommended).

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `healthCheck.active.path` | String | none | Active health-check path, appended to the base URI. Example: `/healthz`. |
| `healthCheck.active.intervalSec` | Integer | `10` | Probe interval in seconds. |
| `healthCheck.active.timeoutMs` | Integer | `2000` | Probe timeout in milliseconds. |
| `healthCheck.active.healthyThreshold` | Integer | `1` | Consecutive successes to mark healthy. |
| `healthCheck.active.unhealthyThreshold` | Integer | `3` | Consecutive failures to mark unhealthy (used only when passive is disabled). |
| `healthCheck.passive.unhealthyThreshold` | Integer | `3` | Consecutive failures to mark unhealthy. |
| `healthCheck.passive.failureStatusCodes` | Array\<Integer\> | `[500, 502, 503, 504]` | HTTP status codes counted as failures. |
| `healthCheck.passive.countTimeout` | Boolean | `true` | Count timeouts as failures. |
| `healthCheck.passive.backoff.initialSec` | Integer | `5` | Initial backoff in seconds (effective only when active is disabled). |
| `healthCheck.passive.backoff.multiplier` | Number | `2.0` | Backoff multiplier. |
| `healthCheck.passive.backoff.maxSec` | Integer | `60` | Maximum backoff in seconds. |

#### Example: Resilient webhook with retry and health checks

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      type: uri
      uri: "https://key-service.internal/resolve"
    requestMethod: POST
    timeoutMs: 3000
    auth:
      type: hmac
      secretRef:
        name: webhook-hmac
    allowDegradation: false
    statusOnError: 503
    retry:
      maxRetries: 1
      retryDelayMs: 100
      retryOnTimeout: true
      retryOnConnectError: true
      retryOnStatus: [502, 503, 504]
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
```

#### Example: Webhook targeting a Kubernetes Service

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver-svc
  namespace: production
spec:
  type: webhook
  config:
    target:
      type: service
      name: key-service
      namespace: production
      port: 8080
    requestMethod: POST
    timeoutMs: 3000
```

## Kafka connector

⚠️ **Kafka is currently a placeholder.** The schema is valid and the resource reports `Accepted=True`, but the Gateway has not yet integrated a Kafka provider. Runtime availability depends on a later release.

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: events-kafka
  namespace: production
spec:
  type: kafka
  config:
    brokers:
      - "kafka-1.production.svc:9092"
      - "kafka-2.production.svc:9092"
      - "kafka-3.production.svc:9092"
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `brokers` | Array\<String\> | **Required** | Kafka broker addresses (`host:port`); list multiple brokers for a cluster. |

## Common configuration scenarios

### 1. Minimal Redis for development

A single-node Redis without authentication or TLS, suitable for quick verification.

⚠️ **For development only! Do not use in production!**

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: dev-redis
  namespace: edgion-test
spec:
  type: redis
  config:
    endpoints:
      - "redis://redis:6379"
```

### 2. Centralized credentials in a dedicated namespace

Store all connector Secrets in a `credentials` namespace and authorize access with a `ReferenceGrant`.

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-linksys-secrets
  namespace: credentials
spec:
  from:
    - group: edgion.io
      kind: LinkSys
      namespace: production
  to:
    - group: ""
      kind: Secret
---
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: cache-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "rediss://redis.production.svc:6379"
    auth:
      secretRef:
        name: redis-auth
        namespace: credentials
    tls:
      enabled: true
```

### 3. TLS-secured Etcd with mTLS client certificate

The CA and client certificate are inlined (public material); the client private key comes from a Secret.

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: config-etcd
  namespace: production
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1.production.svc:2379"
      - "https://etcd-2.production.svc:2379"
    auth:
      secretRef:
        name: etcd-creds
    tls:
      enabled: true
      certs:
        caCert: |
          -----BEGIN CERTIFICATE-----
          <base64-encoded-cert>
          -----END CERTIFICATE-----
        clientCert: |
          -----BEGIN CERTIFICATE-----
          <base64-encoded-cert>
          -----END CERTIFICATE-----
        secretRef:
          name: etcd-client-tls
```

## FAQ

### Q1: Why does my LinkSys report `Accepted=False/Invalid`?

**A**: The structure or per-backend validation failed. Common causes: an empty `endpoints` list, `topology.mode=sentinel` without a `sentinel` block (or `mode=cluster` without `cluster`), a webhook `target.uri` that is not `http://` or `https://`, or a webhook `requestMethod` that is not a valid HTTP method. The status `message` concatenates the specific errors.

### Q2: My credentials look correct, but `ResolvedRefs` is `False`. Why?

**A**: The Secret could not be resolved. Check the reason:

- `SecretNotFound` — the Secret does not exist yet. The Controller automatically requeues when the Secret is created.
- `RefNotPermitted` — a cross-namespace reference without a matching `ReferenceGrant`.
- `InvalidSecretType` — the `Secret.type` is not in the resolver's allowlist for that auth type.
- `MissingKeys` — the Secret is missing required keys (for example basic auth missing `password`, or apiKey missing `apiKeyId` / `apiKeySecret`).

### Q3: Can I put the password directly in the LinkSys YAML?

**A**: No. The auth and TLS structs enable `deny_unknown_fields`, so inline sensitive fields (`username`, `password`, `bearerToken`, `apiKeyId`, `hmacKey`, `clientKey`, ...) cause parsing to fail rather than being ignored. Always use a `secretRef`.

### Q4: Why does setting `insecureSkipVerify: true` break my Redis or Etcd connector?

**A**: Redis and Etcd clients do not support skipping verification — setting `true` rejects startup outright. This is intentional, to avoid a configuration where the field appears set but verification still happens. To connect to a backend with a private CA, provide the CA via `tls.certs.caCert` or a `ca.crt` Secret key instead.

### Q5: What happens to active connections when I rotate a Secret?

**A**: The connector rebuilds transparently. When a referenced Secret changes, the Controller recomputes the credentials, pushes the new spec to the Gateway, and the Gateway rebuilds the client while old connections drain gracefully.

### Q6: Does Edgion fall back to an anonymous connection if credentials fail to resolve?

**A**: No. If a required credential is pending or invalid, the Gateway fails fast and does not build the client. It never degrades to an unauthenticated connection.

## Get help

If you run into problems:

1. Check the FAQ section of this document.
2. Inspect the resource status: `kubectl get linksys <name> -o yaml` and read the `Accepted` / `ResolvedRefs` conditions.
3. Check the gateway logs.
4. Open an issue on GitHub.

## Related documents

- [Secret Management](./secret-management.md)
- [Cross-Namespace Reference](./reference-grant.md)
- [mTLS Configuration](./mtls.md)
- [Redis-backed Rate Limit](../../user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md)
- [Access Log](../observability/access-log.md)
