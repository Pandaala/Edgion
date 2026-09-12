# Connectors (LinkSys)

> **🔌 Edgion extension**
> 
> Connectors are external-system integrations defined by the `LinkSys` CRD (`edgion.io/v1`); they are not part of standard Gateway API.

## Overview

A **connector** is a reusable, named handle to an external system — Redis, Etcd, Elasticsearch, an HTTP webhook, Kafka, or an OTLP/gRPC collector. You declare it once as a `LinkSys` resource, attach its credentials through Kubernetes Secrets, and then reference it from any feature that needs that backend. This keeps connection details and credentials in one place instead of duplicating them across every plugin and policy.

A single `LinkSys` object describes exactly one connector. The top-level `spec.type` selects the backend, and `spec.config` carries the configuration for that backend. Several Edgion features consume connectors by `namespace/name`:

- Distributed (cluster-wide) rate limiting references a `redis` connector by `namespace/name` — see [Redis-backed Rate Limit](../../user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md).
- Async log sinks reference an `elasticsearch`, `kafka`, or `otlp` connector — see [Access Log](../observability/access-log.md).
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
| basic (Redis / Etcd / ES) | `kubernetes.io/basic-auth` or `Opaque` | `password` | `username` |
| apiKey (Elasticsearch) | `Opaque` | `apiKeyId`, `apiKeySecret` | — |
| bearer (Elasticsearch / OTLP) | `Opaque` | `bearerToken` | — |
| TLS (all backends) | `kubernetes.io/tls` or `Opaque` | At least one of `tls.crt` / `tls.key` / `ca.crt` | — |

Application-layer key names use camelCase; TLS keys follow the Kubernetes `kubernetes.io/tls` standard.

> **Webhook credentials are different.** The webhook connector no longer has a typed `auth` block. Instead, you inject any credential as a custom request header (or arg/cookie) that reads from a Secret. The Secret key name is **operator-chosen** (whatever you put in `key:`), and an optional `prefix:` lets you prepend `Bearer ` / `Basic ` / etc. See [Webhook connector → Custom field sources](#custom-field-sources).

### TLS

Every backend accepts the same `tls` block for securing the connection.

```yaml
tls:
  enabled: true
  verify: true                 # verify peer certificate (default true); false is honored (operator responsibility)
  validation:                  # optional; omit to use built-in webpki root CAs
    caCertificateRefs:         # list of CA cert refs; for LinkSys, only kind: Secret is supported
      - kind: Secret
        name: my-ca-bundle     # Secret must have a ca.crt key (type kubernetes.io/tls or Opaque)
    wellKnownCACertificates: System  # alternatively, explicitly use built-in webpki roots
    # Phase 1: hostname and subjectAltNames are schema-exposed but not enforced.
    # Configuring them causes Accepted=False until Phase 2.
  clientCertificateRef:
    name: my-client-tls        # Secret holding tls.crt + tls.key for client mTLS
```

> **Phase 1 CA ref limitation**: `validation.caCertificateRefs` for LinkSys (redis / etcd / elasticsearch / kafka / otlp) only supports `kind: Secret` in this release. ConfigMap CA is not supported for LinkSys. If you need ConfigMap CA support, use the plugin (OIDC / LDAP / ForwardAuth / EGC) TLS paths. Inline CA (`caCert`) has been removed; use a Secret with a `ca.crt` key instead.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | Boolean | `false` | Enable TLS for the connection. For URLs that already imply TLS (`https://`, `rediss://`), this only adds custom CA / client-certificate handling. |
| `verify` | Boolean | `true` | Verify the peer TLS certificate when the block is enabled. `verify: false` is honored (operator responsibility); a WARN is logged at startup. Redis, Etcd and OTLP reject `verify: false` in enabled blocks — their SDKs have no skip-verify API. |
| `validation` | Object | none | Optional trust anchor configuration. Omit to use built-in webpki root CAs. |
| `validation.caCertificateRefs` | Array | none | List of CA certificate Secret refs (each: `{ kind: Secret, name, namespace }`). The Secret must have a `ca.crt` key. For LinkSys, only `kind: Secret` is supported in this release. A custom CA **replaces** the built-in roots. |
| `validation.wellKnownCACertificates` | String | none | Explicitly use built-in webpki root CAs. Allowed value: `System`. |
| `clientCertificateRef` | Object | none | Kubernetes Secret holding the client mTLS identity (`tls.crt` / `tls.key` keys). |

The client mTLS identity (certificate + private key) must be provided via `clientCertificateRef`.

When `tls.enabled` is `false` (including its default), the block's verification
settings and certificate references are inactive. Missing or invalid referenced
Secrets do not prevent the connector from becoming ready. Disabling the block
also clears previously resolved TLS certificates and their dependencies; enabling
it again requires those references to resolve. Authentication Secrets remain
required independently. A secure endpoint URL such as `https://` or `rediss://`
still selects TLS, using the provider's normal defaults instead of this inactive
block. The same rule applies to the TLS block of a `credentialSource` provider.

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
| `topology.cluster.readFromReplicas` | Boolean | none | Allow reads to be served from replica nodes. |
| `topology.cluster.maxRedirects` | Integer | none | Maximum cluster redirects to follow. |

When `topology` is omitted, the connector runs in standalone mode. When `mode=sentinel` but `sentinel` is missing (or `mode=cluster` but `cluster` is missing), the status reports `Accepted=False/Invalid`.

> **Note**: `topology.sentinel.password` (a Sentinel-specific password separate from Redis node auth) has been removed — it was a runtime no-op and was never consumed by the connector. If you have an existing CR that includes a `password:` field under `topology.sentinel`, the Kubernetes API server will silently prune it on re-apply; it does not cause a parse error.

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
      clientCertificateRef:
        name: etcd-client-tls
    namespace: "/my-app/"
```

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoints` | Array\<String\> | **Required** | Etcd endpoints; at least one. Example: `"https://etcd-1:2379"`. |
| `auth` | Object | none | Authentication. The Secret typically provides both `username` and `password`. |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). Etcd (like Redis) rejects `verify: false` in enabled TLS blocks in any environment. |
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
| `bulk.maxRetries` | Integer | `3` | Shared retry budget for transport failures, HTTP 5xx responses, and item-level 429 rejections. |
| `bulk.backoffMs` | Integer | `1000` | Initial backoff between retries in milliseconds. |
| `bulk.maxBodyBytes` | Integer | `10485760` (10 MB) | Maximum bulk request body size in bytes. Ceiling 50 MB. |
| `bulk.channelSize` | Integer | `10000` | Producer-to-bulk-loop channel capacity. Ceiling 100000. |

For a successful HTTP Bulk response with item-level failures, the connector retries only items
whose status is `429`, preserving their request order. It uses `bulk.backoffMs` as the initial
delay and the existing exponential backoff; transport, HTTP 5xx, and item-level retries share the
same `bulk.maxRetries` budget. Later log entries remain in the bounded `bulk.channelSize` queue,
up to its configured capacity, while that serial retry is in progress. Once the queue is full,
new entries are dropped and counted by `edgion_access_log_dropped_total`; that failed
submission does not fall through to the fallback. Other item failures are permanent. An unsafe
response correlation (for example, a mismatched item count) is not retried because resending an
ambiguous operation could duplicate a document. If the 429 budget is exhausted, the connector
becomes unhealthy and a configured access-log fallback can receive new entries.

### Failed-document cache

> **Status: planned — not yet implemented.** The `failedCache` schema and example
> below describe the intended design only. The gateway does **not** currently honor
> a `failedCache` block (unknown CRD fields are pruned, so setting it has no effect).
> Until it ships, register a `localFile` sink as a fallback so logs are not lost
> during an Elasticsearch outage. Tracking: `edgion-gateway/src/link_sys/readme`.

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

> ⚠️ **Breaking: `successStatusCodes` has been replaced by `success.statusCodes`.** `successStatusCodes` is no longer a recognized runtime field. A webhook YAML that still sets it remains admitted, serde drops the field, and success falls back to "any 2xx". The Controller now reports the exact path through the advisory unknown-field warning, `UnknownConfigFields` condition, diagnostics/metric surfaces, and Admin API write/validate responses; the finding does not change `Accepted` or block sync. Audit every webhook LinkSys (and every ForwardAuth plugin, which shares this connection type) and migrate to the nested `success:` block now; see [Success](#success) below.

A webhook connector lets plugins (ctx-set, key-auth, hmac-auth, and so on) call an external HTTP service for user resolution, permission checks, context enrichment, audit logging, and similar lookups. The connector is the **single source of truth** for the outbound request: it declares the backend (the `name`/`namespace`/`port`/`kind`/`group` fields set directly under `target`), and a `request` block that defines exactly which path, method, query args, headers, and cookies are sent. Consuming plugins can no longer silently reshape the request — they can only feed values into the named slots the webhook author declared, optionally override the path (when allowed), and parse the response. Outbound fields follow an **allow-list** model: nothing is forwarded unless the webhook explicitly names it, so a misbehaving upstream plugin cannot leak headers the operator never intended to expose.

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
      kind: Service
      name: key-service
      namespace: production
      port: 8080
    request:
      method:
        template: POST
      headers:
        origin:
          - name: X-Tenant-Id
        custom:
          - name: X-Source
            value: edgion
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token
              key: token
              prefix: "Bearer "
    timeoutMs: 3000
    success:
      statusCodes: [200]
    maxResponseBytes: 2048
```

### Target

Every webhook references its backend by setting Service fields directly under `target` — the same field set (`group`/`kind`/`name`/`namespace`/`port`) as Gateway API's `BackendObjectReference`, flattened with no nested wrapper key. There is no longer a `type: uri` / `type: service` choice, and there is no `backendRef` key — the flat Service fields (or the `url` form below) are the only target shapes.

```yaml
target:
  group: ""              # optional; defaults to the core API group
  kind: Service          # Service | ServiceClusterIp | ServiceEndpointSlice (empty defaults to Service)
  name: webhook-backend
  namespace: production  # optional; defaults to the LinkSys namespace
  port: 8080             # required
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target.group` | String | core (`""`) | API group of the referenced object. |
| `target.kind` | String | `Service` | Target kind. Allowed: `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, or empty (defaults to `Service`). |
| `target.name` | String | **Required** | Kubernetes Service name. |
| `target.namespace` | String | LinkSys namespace | Service namespace. |
| `target.port` | Integer | **Required** | Service port; must be greater than 0. A missing port is rejected at admission with `Accepted=False`. |

Resolution by kind:

- `Service` (default, and empty/unset): the gateway resolves ready endpoints from the Kubernetes EndpointSlice store at call time.
- `ServiceEndpointSlice`: explicit form of the same EndpointSlice resolution.
- `ServiceClusterIp`: resolved via the Service ClusterIP DNS name (kube-proxy).

`kind: ExternalName` and `ServiceImport` are **not** supported as webhook targets and are rejected at admission with a descriptive `Accepted=False` message. To reach an external endpoint, create a proxy Service (for example a `LoadBalancer` Service, or a `ClusterIP` Service fronted by an ingress controller) that routes to the external host, then reference that Service by name — or use the URL target below instead.

### URL target (external services)

Instead of the Service fields, a webhook can address an arbitrary external URL directly:

```yaml
target:
  url: https://api.example.com
  blockPrivate: true       # optional; URL targets only, see "Egress policy" below
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target.url` | String | none | External URL target. **Origin only**: `scheme://host[:port]` — no path, query, fragment, or credentials. Mutually exclusive with `target.name` / `target.namespace` / `target.port` / `target.kind` / `target.group`. |
| `target.blockPrivate` | Boolean | `false` (inherits the gateway-local default) | URL targets only. See [Egress policy](#egress-policy-ssrf-protection). |

**Origin-only, and why.** `target.url` must not carry a path, query string, fragment, or embedded
credentials — setting any of those is rejected at admission with `Accepted=False`. The outbound path
is decided exclusively by `request.path.template` (see [Outbound request](#outbound-request) below),
which already composes correctly with `healthCheck.active.path` and a consumer's `pathOverride`. If
`target.url` also carried a path, every one of those composition rules would need a second, competing
definition of "the base path" — origin-only removes the ambiguity instead of documenting around it.
`https://api.example.com` with `request.path.template: /v1/check` is the canonical shape.

**Mutually exclusive with the Service fields.** Setting both `target.url` and any of
`target.name` / `target.namespace` / `target.port` / `target.kind` / `target.group` is rejected at
admission — a webhook addresses either an external URL or a Kubernetes Service, never both.

**`http://` + `tls.enabled: true` is rejected.** If `target.url` starts with `http://` (case-insensitive
scheme check) and the connection's `tls.enabled` is `true`, admission fails with `Accepted=False`.
This is deliberate: a URL target's scheme already decides plaintext vs. TLS, so an explicit
`tls.enabled: true` alongside `http://` would either be silently ignored or silently downgrade the
operator's TLS intent — neither is acceptable, so it is rejected instead of guessed.

**Dedicated connection pool.** A URL target always builds its own `reqwest::Client` — including when
no `tls` block is set at all — and never falls back to the shared process-global HTTP client that
other consumers use. This is what lets each URL target own its own DNS resolution and address cache
(see [Egress policy](#egress-policy-ssrf-protection)); it also means URL targets do not share
connection-pool exhaustion or health state with unrelated webhooks.

### Egress policy (SSRF protection)

A URL target is addressed by hostname, so — unlike a Service target, which is resolved via
Kubernetes EndpointSlice — the gateway performs its own DNS resolution and must classify the
result before connecting. Two enforcement points implement one policy, so the behavior is the same
whether `target.url` names a hostname or an IP literal:

- **Always blocked**: loopback (`127.0.0.0/8`, `::1`) and link-local addresses, which includes the
  cloud-metadata address `169.254.169.254` and its whole `169.254.0.0/16` segment.
- **Blocked when `target.blockPrivate: true`**: RFC1918 private ranges (`10/8`, `172.16/12`,
  `192.168/16`) and unique-local (`fd00::/8`) addresses, in addition to the always-blocked classes.
- **Default (`blockPrivate` unset or `false`)**: private ranges are allowed, matching the long-standing
  behavior for Service targets — most webhooks legitimately point at an internal artifact repository
  or SaaS reachable only over a private network.

A rejected address surfaces as a connect failure for that call (WebhookKeyGet writes no value, same
as any other webhook failure). Rejections increment the
`edgion_outbound_egress_blocked_total{reason="private"|"loopback"|"link_local"}` counter, but only
on the **hostname** path (`EgressResolver`, which records all three reasons). On the **IP-literal**
path, only `reason="private"` is ever recorded: the `dial_url` `blockPrivate` check records that
metric, but the loopback/link-local rejection there goes through the shared backend guard
(`validate_endpoint_backend_addr`) that predates this metric and does not touch it. So a dashboard
or alert built on this counter sees full coverage for a hostname target but is blind to
loopback/link-local rejections when `target.url` is an IP literal.

**Gateway-local default.** When a webhook does not set `target.blockPrivate` explicitly, the effective
value comes from the gateway's own configuration:

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `outbound_egress_block_private` (gateway config file) / `EDGION_OUTBOUND_BLOCK_PRIVATE` (env) | Boolean | `false` | Gateway-wide default for `target.blockPrivate` when a webhook omits it. Truthy values: `1`, `true`, `yes`, `on` (case-insensitive). |
| `outbound_egress_dns_cache_ttl_ms` (gateway config file) / `EDGION_OUTBOUND_DNS_CACHE_TTL_MS` (env) | Integer (ms) | `5000` | How long a URL target's resolved-and-classified addresses are cached before being re-resolved. An unparseable env value is ignored (a WARN is logged) and the configured value is kept. |

**Precedence: the resource field wins.** An explicit `target.blockPrivate` on a webhook always
overrides the gateway-local default — a cluster administrator who wants the stricter posture
everywhere flips `outbound_egress_block_private`; an individual webhook that needs to reach an internal
`10.x` artifact repository can still opt out with `target.blockPrivate: false`, and vice versa.

**No command-line flag, on purpose.** Both settings are config-file / environment-variable only —
there is no `--outbound-egress-block-private` CLI flag. The gateway's config-merge logic only carries
a small, fixed set of fields from the CLI into a file-loaded configuration, and `--config-file`
defaults to a real path in the common deployment, so a flag here would silently have no effect in
that path. Set these via the gateway config file or the `EDGION_*` environment variables instead.

**This is defense-in-depth, not the primary control.** `target.blockPrivate` and
`outbound_egress_block_private` narrow what an already-privileged LinkSys/EdgionPlugins write can reach;
they are not a substitute for network-level isolation. The primary egress control remains a
Kubernetes `NetworkPolicy` on the Gateway Pod restricting which external IPs/ports it may reach at
all — the code-layer guard exists because a URL target lets an operator name an arbitrary external
host by hostname, something a NetworkPolicy alone cannot evaluate at request-authoring time.

### Outbound request

The `request` block declares the outbound request shape. All five sub-fields are optional:

- **`path`** / **`method`** — single-valued. Omit either to reuse the **frozen** value from the incoming client request. When present, the value is a `template` (a literal string or a `${type:name}` expression).
- **`args`** / **`headers`** / **`cookies`** — allow-listed field groups, each split into an `origin` list and a `custom` list. Omit a group to forward nothing of that kind. Cookies are forwarded as a single `Cookie` header.

```yaml
request:
  path:
    template: "/resolve/${ctx:action}"
    allowOverride: true        # consumer may pass pathOverride at call time
  method:
    template: POST

  args:
    origin:
      - name: tenant_id                      # missing → omitted (default)
      - name: trace_id
        presence: required                   # missing → webhook call fails
      - name: org_id
        presence: warnIfMissing              # missing → PluginLog alert, continues
    custom:
      - name: source
        value: edgion                        # literal
      - name: action
        template: "${ctx:action}"            # evaluated at call time

  headers:
    origin:
      - name: X-Tenant-Id
      - name: X-User-Id
        presence: required
    custom:
      - name: X-Caller
        value: edgion-gateway
      - name: X-User
        template: "${ctx:user_id}"
      - name: Authorization
        secret:
          secretRef:
            name: webhook-token
          key: token
          prefix: "Bearer "

  cookies:
    origin:
      - name: session_id
    custom:
      - name: webhook_call_id
        template: "${ctx:trace_id}"
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `request.path.template` | String | frozen incoming path | Outbound path; literal or `${type:name}` template. |
| `request.path.allowOverride` | Boolean | `false` | When `true`, a consumer's `pathOverride` replaces the whole path at call time. When `false`, any `pathOverride` is ignored. |
| `request.method.template` | String | frozen incoming method | Outbound HTTP method; literal (`POST`, `GET`, …) or template. |
| `request.{args,headers,cookies}.origin` | Array | `[]` | Fields copied from the **frozen** incoming request by name. |
| `request.{args,headers,cookies}.custom` | Array | `[]` | Operator-constructed fields (literal / template / secret). |

**Origin fields** copy the same-named value from a frozen entry-time snapshot of the incoming request — immune to any rewrite made by an earlier plugin in the chain. Each origin field is just `{ name, presence }`. There are three kinds of origin group, one per HTTP location:

- `headers.origin` — copies request headers. **Header names are case-insensitive** (HTTP standard).
- `args.origin` — copies query-string parameters. **Arg names are case-sensitive.**
- `cookies.origin` — copies individual cookies. **Cookie names are case-sensitive.**

If you instead want the **current** (possibly rewritten) view of a header, use a custom template field: `custom: [{ name: X-Foo, template: "${header:X-Foo}" }]`.

#### Presence policies

`presence` lives only on `origin` fields (custom fields always have a value). It controls what happens when the named field is absent from the incoming request:

| Policy | Behavior when the field is missing | When to use |
|--------|-----------------------------------|-------------|
| `skipIfMissing` (default) | The field is silently omitted from the outbound request; no log. | Optional enrichment where absence is normal. |
| `warnIfMissing` | The field is omitted, a PluginLog alert is written, and the request continues. | Fields that *should* be present — you want visibility without failing the call. |
| `required` | The webhook call is treated as failed: WebhookKeyGet writes no value, same as any other webhook failure. | Fields the backend cannot function without. |

Missing `warnIfMissing` fields share one `warn outbound field missing` PluginLog entry per outbound call, within the normal plugin-log capacity limit. The entry contains no field names or values, and retries do not repeat it. WebhookKeyGet resolves each variable with a separate call. A warning already observed is retained if a later build step or the external exchange fails; calls stopped before field evaluation produce no missing-field warning.

`statusOnError` is declared on this connector's schema (it shares `ExternalHttpConnection` with the ForwardAuth plugin), but the **webhook connector itself does not read it**: on any failure — including a missing `presence: required` field — WebhookKeyGet writes no value (or its `default`), and the request continues. Setting this field on a webhook `LinkSys` has no effect; it is meaningful only when the same struct is embedded directly in a ForwardAuth plugin config.

#### Custom field sources

Each `custom` entry is `{ name, <source> }` where the source is **exactly one** of:

| Source | Form | Resolved | Notes |
|--------|------|----------|-------|
| Literal | `value: "edgion"` | n/a | Verbatim static string. |
| Template | `template: "${ctx:user_id}"` | call time | Interpolated by the ctx-set template engine (see [Template variables](#template-variables)). |
| Secret | `secret: { secretRef: {...}, key: ..., prefix: ... }` | config time | Read from a Kubernetes Secret by the controller and shipped to the gateway over conf_sync. The resolved value is redacted in `/configclient` output. |

The Secret form is how you inject credentials (there is no typed `auth` block anymore):

```yaml
custom:
  - name: Authorization
    secret:
      secretRef:
        name: webhook-token       # namespace defaults to the LinkSys namespace
      key: token                  # operator-chosen key inside the Secret's data map
      prefix: "Bearer "           # optional; prepended to the resolved value
```

`prefix` is what turns a raw token into `Bearer <token>`, `Basic <encoded>`, and so on. Secret rotation is automatic: when the referenced Secret changes, the controller requeues the LinkSys resource, re-resolves the value, and re-emits it to the gateway — no operator action required.

#### Template variables

Templates in `path.template`, `method.template`, and `custom[].template` reuse the ctx-set template engine. They are pre-compiled at resource ingest, so the hot path only performs substitution. Syntax is `${type:name}`:

| Token | Meaning |
|-------|---------|
| `${header:Name}` | Incoming request header (current view). |
| `${query:name}` | Incoming request query parameter. |
| `${cookie:name}` | Incoming request cookie. |
| `${ctx:name}` | Gateway context variable set by ctx-set or another plugin. |
| `${path}` / `${uri}` | Current (possibly rewritten) path / URI. |
| `${requestPath}` / `${requestUri}` | Frozen entry-time path / URI. |
| `${method}` | Current HTTP method. |
| `${remoteIp}` | Resolved client IP. |

A `${ctx:X}` that names an undefined variable resolves to an empty string. To enforce presence, use `presence: required` on an origin field, or set the ctx variable in a ctx-set step before calling the webhook.

#### Path override

When `request.path.allowOverride: true`, a consuming plugin may pass `pathOverride` to replace the entire path at call time. When `allowOverride` is `false` (the default) or the `path` block is omitted, any `pathOverride` the consumer sends is **ignored**. This keeps the webhook resource in control of its own surface while still allowing per-call paths where the author opts in.

#### Field-conflict validation

The following combinations are rejected at admission with `Accepted=False`:

- The same name appearing in both `origin` and `custom` within one group (e.g. `headers.origin[name: X-Foo]` and `headers.custom[name: X-Foo]`).
- A duplicate name within a single `origin` list or a single `custom` list.
- Declaring both `headers.origin[name: Cookie]` and any non-empty `cookies.*` list (would double-inject the `Cookie` header) — use one or the other.

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timeoutMs` | Integer | `5000` | Request timeout in milliseconds; must be greater than 0. |
| `timeoutMsTemplate` | String | none | Template rendering a per-request timeout in milliseconds, e.g. `"${ctx:desired_timeout_ms}"`. Restricted to a trusted variable allow-list — see [Templates](#templates) below. |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). Defaults to the system CA bundle when unset. |
| `success` | Object | any 2xx | Success criteria: status codes plus optional JSON body predicates. **Replaces the removed `successStatusCodes`** — see [Success](#success) below. |
| `maxResponseBytes` | Integer | `1024` | Maximum response body size to read in bytes. Clamped at call time to `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes` (default 16 KiB). Must be `>= 4096` when `success.body` predicates are set — see [Success](#success). |

> **Removed fields.** `requestMethod`, `auth`, and `defaultHeaders` are deprecated. They are still accepted during a rolling upgrade but ignored at runtime, and the controller logs a WARN when it sees them. They will be removed in the next release — migrate before then. See [Migration from the old schema](#migration-from-the-old-schema).
>
> `successStatusCodes` is different: it is not on this deprecation path and has no runtime compatibility shim. It is ignored, but the Controller reports it through the advisory unknown-field surfaces described above. See [Success](#success) below.

### Success

Replaces the old `successStatusCodes` field (removed; pre-release, no alias — see the warning at the top of this section). A response is classified as success only when **both** the status check and the optional body predicates pass:

```yaml
success:
  statusCodes: [200]          # omitted -> any 2xx (200-299), same as before
  body:                       # optional; ANDed with statusCodes
    - pointer: "/code"        # RFC 6901 JSON Pointer into the parsed response body
      equals: 0
    - pointer: "/data/status"
      in: ["ok", "accepted"]
```

Motivation: some backends always return `200 OK` and put the real outcome in the body, e.g. `{"code": -101, "message": "invalid user"}`. Without a body predicate, the gateway treats that as success and retry / degradation never trigger.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `success.statusCodes` | Array\<Integer\> | any 2xx | Status codes considered successful. Omitted → any 2xx (200-299). An explicit empty list (`statusCodes: []`) is **rejected at admission** rather than silently meaning "any 2xx" — the same effect as omitting the field with none of the visibility, which would let e.g. a Helm template that renders an empty list widen an auth allow-list unnoticed. |
| `success.body` | Array\<Object\> | `[]` | JSON-Pointer body predicates, ANDed with the status check and with each other. |
| `success.body[].pointer` | String | **Required** | RFC 6901 JSON Pointer into the parsed response body, e.g. `/code` or `/data/status`. |
| `success.body[].equals` / `.notEquals` / `.exists` / `.in` | — | — | Exactly one operator per entry (admission-enforced at deserialize time — more than one, or none, fails to parse). `equals` / `notEquals` / `in` compare as strict `serde_json::Value` — **no type coercion**: `equals: 0` matches JSON `0` but not the string `"0"`. `exists: true/false` checks presence only. `in` takes a non-empty list of values. |

**Non-JSON body → failure, not an error.** When any `success.body` predicate is configured and the response body is not valid JSON (wrong `Content-Type` or unparseable), the response is classified as a **failure** — no panic, no error surfaced to the caller.

**Missing pointer → failure, except for `exists: false`.** When the body IS valid JSON but a predicate's `pointer` resolves to nothing, the predicate fails for `equals`/`notEquals`/`in` — you cannot assert an equality/membership claim about a field that isn't there. The one legitimate exception is `exists: false`, which is defined to *mean* "the pointer is absent" and therefore succeeds in exactly that case. For example, `{pointer: /error, notEquals: true}` against a `200 {"authenticated": false}` body (no `/error` key at all) is a **failure**, not a pass — a business error hiding behind a differently-named field must not be able to slip through a `notEquals` check aimed at a field it doesn't populate.

**`maxResponseBytes` must be raised when using body predicates.** The default cap is 1024 bytes (1 KiB). If `success.body` is non-empty, admission requires `maxResponseBytes >= 4096`; otherwise the resource is rejected with `Accepted=False`. This exists because a body larger than the cap is truncated *before* classification — and a truncated body surfaces as a distinct `ResponseTooLarge` failure, **not** as "the predicate failed". Distinguish the two when debugging: a `ResponseTooLarge` failure means raise `maxResponseBytes`; a predicate failure means the backend's response genuinely did not match.

**Not evaluated by passive health.** Passive health checks (`healthCheck.passive.failureStatusCodes`) still key off the HTTP status only — a business-level error in a 200 body does not trip the connector's health state or its circuit breaker.

### Retry

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `retry.maxRetries` | Integer | `1` | Maximum retries (not counting the initial call). Range `0..=10` — a larger value is rejected at admission (`Accepted=False`). This exists alongside `maxDelayMs`'s ceiling below: an unbounded `maxRetries` (e.g. 200) would still allow tens of minutes of inline `tokio::time::sleep` on a live data-plane request even with a capped per-attempt backoff. |
| `retry.retryDelayMs` | Integer | `100` | **Base delay of the backoff curve** in milliseconds — not a fixed "retry interval". Under `backoffPolicy: fixed` it *is* the constant delay; under `exponential` (default) it is the first term of the curve. Must be greater than 0. |
| `retry.backoffPolicy` | Enum | `exponential` | `exponential` \| `fixed`. `exponential`: `delay = min(maxDelayMs, retryDelayMs << (attempt-1))`. `fixed`: `delay = retryDelayMs` (constant every attempt). |
| `retry.maxDelayMs` | Integer | `1000` | Per-attempt backoff ceiling in milliseconds, applied **before** jitter. Range `1..=10000` — a larger value is rejected at admission (`Accepted=False`). The retry sleep blocks a live client request inline on the data-plane path, so this is a hard safety cap, not a tuning knob for waiting out an outage (that is the health-check / circuit-breaker's job). |
| `retry.jitter` | Boolean | `true` | Add 50–100% jitter to each backoff delay (`d = uniform(d/2 ..= d)`, applied *after* the `maxDelayMs` cap, so the result never exceeds it). Protects against thundering-herd retries. |
| `retry.honorRetryAfter` | Boolean | `false` | Opt-in. When the response status is `429` or `503` **and** that status is already listed in `retryOnStatus`, honor the response's `Retry-After` header (both `delta-seconds` and HTTP-date forms) in place of the computed backoff. A parsed value `<= maxDelayMs` is used as this attempt's delay; a value `> maxDelayMs` makes the gateway **stop retrying** and return the backend's response as-is — it does not retry immediately and does not error. An absent or unparseable `Retry-After` falls back to the computed backoff. `honorRetryAfter` changes *when* a retry happens, never *whether* one happens. |
| `retry.retryOnTimeout` | Boolean | **per-consumer — see below** | Retry on request timeout. |
| `retry.retryOnConnectError` | Boolean | `true` | Retry on connection error (refused, reset). A connect failure means the request never reached the backend, so retrying is always idempotency-safe — this default is unaffected by the consumer split below. |
| `retry.retryOnStatus` | Array\<Integer\> | `[]` | HTTP status codes that trigger a retry. Status retry stays opt-in. |
| `retry.retryOnBodyFailure` | Boolean | `false` | Retry when a `success.body` predicate (see [Success](#success)) classifies the response as a failure. Default off: a business error like `{"code": -101}` will still be `-101` on the tenth attempt — retrying it is pure waste. Enable only when you know the body failure is transient. |

When `retry` is omitted, the webhook fails immediately on the first error.

> **`retryOnTimeout` default changed and is now per-consumer: webhook `false`, ForwardAuth `true`.** A timeout means the outcome is *unknown* — the backend may have already fully processed the request. Webhook calls are overwhelmingly non-idempotent POSTs (send a notification, charge a card, write an audit record), so retrying a timed-out webhook risks double execution — hence `false`. ForwardAuth calls are idempotent authorization checks, where retrying a transient timeout is a genuine availability win — hence `true`. **This is a real behavior change** for any webhook LinkSys that sets a `retry:` block and previously relied on the old unconditional `true` default; an operator can always override the field explicitly. `retryOnConnectError` is unaffected (`true` for both, unchanged) because a connect failure is always safe to retry.

### Templates

One connection-level field can be driven by the same per-request template engine used in `request.*` (see [Template variables](#template-variables)) — but with a **much narrower variable allow-list** than `request.*`. It is compiled and validated at admission; a malformed template, or one that references a disallowed variable type, is rejected with `Accepted=False`.

> ⚠️ **Only `${ctx:...}`, `${remoteIp}`, `${matchedHttpRoute:...}`, and `${matchedGrpcRoute:...}` are accepted in `timeoutMsTemplate`.** `${header:...}`, `${query:...}`, `${cookie:...}`, `${path}`, `${uri}`, `${requestPath}`, `${requestUri}`, `${method}`, and `${secretRef:...}` are all rejected at admission for this field specifically (they remain fully allowed in `request.*`). The field is security-sensitive: `timeoutMsTemplate` sets the auth-service call deadline. A client-controlled source would let an attacker pin the deadline to a value so small the call always times out (a reliable denial of service).

```yaml
timeoutMsTemplate: "${ctx:desired_timeout_ms}"     # falls back to timeoutMs
```

**Rule: the template wins when it renders to a parseable value; otherwise fall back to the static field.**

| Parameter | Renders to | Fallback | Notes |
|-----------|-----------|----------|-------|
| `timeoutMsTemplate` | `u64` milliseconds | `timeoutMs` | A non-numeric, empty, or below-floor render falls back. The floor is `MIN_OUTBOUND_TIMEOUT_MS` = 50 ms (a render below that is treated the same as zero/empty). The rendered value is **clamped to 60000 ms (60 s)** on the upper end regardless. |

Every silent fallback increments `edgion_outbound_template_fallback_total{field="timeout_ms"}` — see [Observability metrics](#observability-metrics) below, since the data plane cannot log the fallback via `tracing`.

**Total blocking time.** The retry sleep and the outbound call are both inline on the request path, so the worst-case block is `(effective_timeout × max_attempts) + Σ backoffs`, each term bounded independently: `timeoutMsTemplate` by the 60 s clamp (and, on the low end, the 50 ms floor), backoff by `maxDelayMs` and its 10 s hard ceiling, and `maxRetries` itself by its own 10-attempt ceiling (see [Retry](#retry) above).

Neither field can redirect the outbound connection — there is no templated `target.url` in this release — and, with the variable allow-list above, neither field can be driven by anything the client sends on the request that reaches this call.

### Observability metrics

The webhook connector (and the ForwardAuth plugin, which shares the same `ExternalHttpConnection`) emits two Prometheus counters for outcomes that would otherwise be invisible on the data plane (which cannot log via `tracing`):

| Metric | Labels | Fires when |
|--------|--------|-----------|
| `edgion_outbound_template_fallback_total` | `field` ∈ `timeout_ms` | A `timeoutMsTemplate` render failed to parse (or rendered to zero/empty) and the connection fell back to the static field. |
| `edgion_outbound_success_predicate_fail_total` | none | A response's status was accepted by `success.statusCodes` but a `success.body` predicate classified it as a failure — the "200 OK but error in the body" case. |

See [Metrics](../observability/metrics.md) for the general Prometheus reference (scrape config, cardinality, Gateway metrics listener).

### Rate limit

Protects the external webhook service from being overwhelmed by the gateway.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rateLimit.rate` | Integer | **Required** (when `rateLimit` is set) | Maximum calls allowed per window. |
| `rateLimit.windowSec` | Integer | `1` | Window size in seconds. |

The limit applies per Gateway client instance, shared by calls using that client,
before outbound retries. It uses two-bucket sliding-window approximation:
the current count plus the decaying previous count must be below `rate` before
admitting a call. It is not an exact rolling request log or a cluster-wide quota.
Windows use monotonic elapsed time at whole-second precision, with a minimum
window of one second. System clock changes do not alter the window. After two
or more window intervals without admissions, expired counts contribute zero.

Concurrent calls update the window and consume a permit together. A delayed
call whose sampled window has already been replaced is rate limited. Each
bucket supports at most 1,099,511,627,775 admissions even if `rate` is larger;
reaching this bound rejects further admissions in that bucket until the window
advances. Exhausting the window identifier also rejects calls rather than
resetting the counter. These semantics also apply to other plugins using the
shared outbound client's `rateLimit` configuration.

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

### Examples

The following examples each focus on one feature. They are runnable LinkSys manifests; trim the fields you do not need.

#### 1. Service and EndpointSlice resolution

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
      kind: Service              # resolves ready endpoints from EndpointSlice
      name: key-service
      namespace: production
      port: 8080
    request:
      method:
        template: POST
    timeoutMs: 3000
---
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver-clusterip
  namespace: production
spec:
  type: webhook
  config:
    target:
      kind: ServiceClusterIp     # resolves via the ClusterIP DNS name (kube-proxy)
      name: key-service
      port: 8080
    request:
      method:
        template: POST
```

#### 2. Origin fields (allow-list forwarding)

Forward selected values from the frozen incoming request, unchanged. With `path` / `method` omitted, the incoming path and method are reused — the common shape for a forward-auth-style webhook.

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: authz, port: 8080 }
    request:
      headers:
        origin:
          - name: X-Tenant-Id
          - name: X-User-Id
      args:
        origin:
          - name: tenant_id
      cookies:
        origin:
          - name: session_id
```

#### 3. Custom literal fields

Inject static values the operator controls.

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: audit-sink, port: 8080 }
    request:
      method:
        template: POST
      headers:
        custom:
          - name: X-Source
            value: edgion-gateway
          - name: X-Env
            value: production
```

#### 4. Custom fields from K8s Secrets

Inject a credential from a Secret. The `prefix` builds the full header value (`Bearer <token>`).

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: key-service, port: 8080 }
    request:
      method:
        template: POST
      headers:
        custom:
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token   # in the LinkSys namespace
              key: token
              prefix: "Bearer "
          - name: X-API-Key
            secret:
              secretRef:
                name: webhook-apikey
              key: apiKey             # no prefix → raw value
```

#### 5. Template variable substitution

Compute outbound values from incoming request data and gateway context at call time.

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: enrich, port: 8080 }
    request:
      path:
        template: "/resolve/${ctx:action}"
      method:
        template: POST
      headers:
        custom:
          - name: X-User
            template: "${ctx:user_id}"
          - name: X-Forwarded-Path
            template: "${requestPath}"     # frozen entry-time path
          - name: X-Client-Ip
            template: "${remoteIp}"
```

#### 6. Presence policies (required / skipIfMissing / warnIfMissing)

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: authz, port: 8080 }
    request:
      headers:
        origin:
          - name: X-Trace-Id                       # default: skipIfMissing
          - name: X-User-Id
            presence: required                     # missing → WebhookKeyGet writes no value
          - name: X-Org-Id
            presence: warnIfMissing                # missing → PluginLog alert, continues
```

#### 7. Path override (allowOverride)

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: user-resolver, port: 8080 }
    request:
      path:
        template: "/v1/users/lookup"
        allowOverride: true        # consumers may replace the path via pathOverride
      method:
        template: POST
```

With `allowOverride: false` (or no `path` block), a consumer's `pathOverride` is ignored and this template (or the frozen incoming path) is used.

#### 8. ctx-set integration (`${ctx:*}` variables)

A consuming plugin sets context variables first, then references them from the webhook's templates. Per-call dynamic values flow exclusively through `${ctx:*}` — there is no separate "invoker arg" concept.

```yaml
# Webhook resource
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: user-resolver
  namespace: production
spec:
  type: webhook
  config:
    target: { kind: Service, name: user-service, port: 8080 }
    request:
      path:
        template: "/resolve/${ctx:action}"
        allowOverride: true
      method:
        template: POST
      headers:
        custom:
          - name: X-User-Id
            template: "${ctx:user_id}"
---
# Consuming plugins on an HTTPRoute filter.
# CtxSet MUST run first: webhook templates read ${ctx:user_id} / ${ctx:action}.
- type: CtxSet
  config:
    vars:
      - { name: user_id, template: "${header:X-User-Id}" }
      - { name: action, value: "get-user" }
- type: WebhookKeyGet
  config:
    vars:
      - name: resolved_user
        webhookRef: production/user-resolver
        pathOverride: "/special/path"   # honored only because allowOverride: true
        extract:
          source: body
          path: "data.user"
```

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
      kind: Service
      name: key-service
      namespace: production
      port: 8080
    request:
      method:
        template: POST
      headers:
        custom:
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token
              key: token
              prefix: "Bearer "
    timeoutMs: 3000
    retry:
      maxRetries: 1
      retryDelayMs: 100          # base delay of the backoff curve, not a fixed interval
      backoffPolicy: exponential # exponential (default) | fixed
      maxDelayMs: 1000           # per-attempt ceiling, range 1..=10000
      jitter: true               # 50-100% jitter, applied after the maxDelayMs cap
      retryOnTimeout: false      # webhook default; forward_auth defaults to true instead
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

### Migration from the old schema

The webhook schema changed in this release. Old fields are accepted during a rolling upgrade but ignored at runtime (the controller logs a WARN), and will be removed in the next release. Migrate every webhook LinkSys before then.

| Old | New |
|-----|-----|
| `target.type: uri` + `target.uri` | flat Service fields (`target.name`/`target.port`/...) to a (proxy) Service — raw URLs are no longer supported |
| `target.type: service` + `target.name/port` | `target.{kind,name,namespace,port}` set directly under `target` (drop the `type` tag) |
| `auth.bearer` / `auth.basic` / `auth.hmac` / `auth.apiKey` | `request.headers.custom` with `secret:` (use `prefix:` for `Bearer `/`Basic `) |
| `defaultHeaders` (static map) | `request.headers.custom` with `value:` per entry |
| `requestMethod: POST` | `request.method.template: POST` (old default was `GET`) |
| consumer `pathSuffix` | `request.path` on the webhook, or consumer `pathOverride` when `allowOverride: true` |
| consumer `forwardHeaders` | `request.headers.origin` on the webhook (centralized allow-list) |

Old:

```yaml
spec:
  type: webhook
  config:
    target:
      type: uri
      uri: "https://key-service.internal/resolve"
    requestMethod: POST
    auth:
      type: bearer
      secretRef:
        name: webhook-token
    defaultHeaders:
      X-Source: edgion
```

New:

```yaml
spec:
  type: webhook
  config:
    target:
      kind: Service
      name: key-service        # for an external endpoint, point this at a proxy Service
      namespace: production
      port: 8080
    request:
      method:
        template: POST
      headers:
        custom:
          - name: X-Source
            value: edgion
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token
              key: token
              prefix: "Bearer "
```

Consumer-side, `pathSuffix` and `forwardHeaders` are removed. The operator-facing consumer is the `WebhookKeyGet` plugin (not a KeyGet `type: webhook` source):

```yaml
# Old
- type: key-auth
  config:
    keys:
      - type: webhook
        webhookRef: production/user-resolver
        pathSuffix: "/special/path"
        forwardHeaders: ["X-Tenant-Id", "X-User-Id"]

# New — declare path/headers on the webhook resource; WebhookKeyGet writes ctx;
# KeyAuth (or any later plugin) reads { type: ctx, name: … }.
# WebhookKeyGet MUST run before KeyAuth; a reversed list is a silent auth bypass.
- type: WebhookKeyGet
  config:
    vars:
      - name: resolved_key
        webhookRef: production/user-resolver
        pathOverride: "/special/path"   # honored only if allowOverride: true on the webhook
        extract:
          source: body
          path: "data.key"
- type: KeyAuth
  config:
    keySources:
      - type: ctx
        name: resolved_key
```

> **External endpoints.** `target.type: uri` and `kind: ExternalName` are not supported. Front the external host with a proxy Service (LoadBalancer, or ClusterIP behind an ingress controller) and reference that Service by name.

### Webhook troubleshooting

**Webhook not resolving / "webhook not found".**
Verify the Service fields under `target` (`name`/`namespace`/`port`/`kind`): the named Service must exist in the resolved namespace and the `port` must be set and valid. A LinkSys with `Accepted=False` is not loaded into the gateway runtime, so any consumer calling it hits the same "webhook unavailable" path. Run `kubectl get linksys <name> -o yaml` and read the `Accepted` condition message.

**`Accepted=False` on a `kind` value.**
Only `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, and empty are valid. `ExternalName` and `ServiceImport` are rejected — use a proxy Service instead.

**Required-field error / key resolves to nothing.**
An `origin` field with `presence: required` was missing from the incoming request, which counts as a webhook failure: WebhookKeyGet writes no value (or its `default`). Confirm the client actually sends the header/arg/cookie (mind case: headers are case-insensitive, args and cookies are case-sensitive), or use `presence: warnIfMissing` to merely log instead of failing.

**Template variable resolves to empty.**
`${ctx:NAME}` returns empty when the variable was never set. Ensure a ctx-set step sets it before the webhook is called, and check the `${type:name}` spelling. To enforce presence, prefer `presence: required` on an `origin` field over a template.

**Path override not working.**
`pathOverride` is honored only when the webhook's `request.path.allowOverride` is `true`. If `path` is omitted or `allowOverride` is `false`, the override is silently ignored and the template (or frozen incoming path) is used.

**Admission error about duplicate or conflicting fields.**
You named the same field in both `origin` and `custom`, listed a duplicate name inside one list, or declared both `headers.origin[name: Cookie]` and a `cookies.*` list. Remove the conflict.

**Health state reset vs. endpoint churn.**
Editing any `request.*` field (or the target) rebuilds the webhook runtime and **resets** its health state — this is intended, an operator config change is not pod churn. EndpointSlice changes (pods scaling/restarting) do **not** rebuild the runtime and do **not** reset health: endpoints are re-resolved at call time while the aggregate health model is preserved.

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

The CA bundle is provided via a Secret ref (`validation.caCertificateRefs`). The client certificate and private key come from a separate Secret (`clientCertificateRef`). Note: inline CA (`caCert`) is no longer supported in this release; use a Secret with a `ca.crt` key instead.

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
      validation:
        caCertificateRefs:
          - kind: Secret
            name: etcd-ca-bundle   # Secret with ca.crt key; e.g. kubectl create secret generic etcd-ca-bundle --from-file=ca.crt=ca.pem
      clientCertificateRef:
        name: etcd-client-tls
```

## FAQ

### Q1: Why does my LinkSys report `Accepted=False/Invalid`?

**A**: The structure or per-backend validation failed. Common causes: an empty `endpoints` list, `topology.mode=sentinel` without a `sentinel` block (or `mode=cluster` without `cluster`), a webhook `target` Service reference with a missing/zero `port` or an unsupported `kind` (only `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, empty are allowed), or a webhook `request` with conflicting fields (the same name in both `origin` and `custom`, a duplicate name within a list, or both `headers.origin[name: Cookie]` and a `cookies.*` list). The status `message` concatenates the specific errors.

### Q2: My credentials look correct, but `ResolvedRefs` is `False`. Why?

**A**: The Secret could not be resolved. Check the reason:

- `SecretNotFound` — the Secret does not exist yet. The Controller automatically requeues when the Secret is created.
- `RefNotPermitted` — a cross-namespace reference without a matching `ReferenceGrant`.
- `InvalidSecretType` — the `Secret.type` is not in the resolver's allowlist for that auth type.
- `MissingKeys` — the Secret is missing required keys (for example basic auth missing `password`, or apiKey missing `apiKeyId` / `apiKeySecret`).

### Q3: Can I put the password directly in the LinkSys YAML?

**A**: No. The auth and TLS structs enable `deny_unknown_fields`, so inline sensitive fields (`username`, `password`, `bearerToken`, `apiKeyId`, `hmacKey`, `clientKey`, ...) cause parsing to fail rather than being ignored. Always use a `secretRef`.

### Q4: Can I disable TLS verification (`verify: false`) for Redis or Etcd?

**A**: No. Redis and Etcd clients do not support disabling certificate verification — `verify: false` in an enabled TLS block is rejected at startup in any environment. A disabled block's `verify` setting is ignored; it cannot disable verification on a URL-implied TLS connection. To connect to a backend with a private CA, enable TLS and use `tls.validation.caCertificateRefs` pointing to a Secret with a `ca.crt` key.

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
- [Metrics](../observability/metrics.md)

## OTLP/gRPC connector

Use `type: otlp` to export log records to an OpenTelemetry Collector. One LinkSys
resource shares its connection, exporter, batch processor and TCP health probe
across access, TCP, TLS and UDP log outputs. SSL logging uses a synchronous file
writer and does not support OTLP.

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  namespace: observability
  name: collector
spec:
  type: otlp
  config:
    endpoint: https://collector.observability.svc:4317
    timeoutMs: 10000
    auth:
      secretRef:
        name: collector-auth
    tls:
      enabled: true
      validation:
        caCertificateRefs:
          - kind: Secret
            name: collector-ca
      clientCertificateRef:
        name: collector-client
```

Create the referenced Secrets in the same namespace: `collector-auth` contains
`bearerToken`, `collector-ca` contains `ca.crt`, and `collector-client` is a
`kubernetes.io/tls` Secret containing `tls.crt` and `tls.key`. Omit `auth` for an
unauthenticated collector. Omit the TLS block to use HTTPS with system roots;
omit only `clientCertificateRef` when client authentication is unnecessary.
Custom CA certificates replace public roots. Cross-namespace Secret references
require a ReferenceGrant. Authentication supports only a Bearer token from a
Secret (at most 8192 bytes); arbitrary metadata and inline credentials are rejected.

Configure the Gateway process output separately:

```yaml
access_log:
  enabled: true
  output:
    otlp:
      linkSysRef:
        namespace: observability
        name: collector
  fallback:
    localFile:
      path: logs/access-fallback.log
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoint` | String | Required | `http://host[:port]` or `https://host[:port]`, optionally ending in `/`. Omitted ports are 80/443; specify `:4317` for a typical collector. |
| `timeoutMs` | Integer | 10000 | Complete export deadline in milliseconds, from 1 through 300000. |
| `auth.secretRef` | Object | None | Secret with the `bearerToken` key. |
| `tls` | Object | None | Shared outbound TLS policy and certificate references. |

Endpoint userinfo, paths other than `/`, queries, fragments, escapes and whitespace
are rejected. IPv6 literals require brackets. Plain HTTP cannot enable a local TLS
block. HTTPS always verifies the original endpoint hostname. Verification disabling,
hostname overrides and SAN overrides are unsupported. A disabled local TLS block
is ignored; an HTTPS endpoint still uses TLS and the global verification policy.

Every connection and health probe applies Gateway egress policy to the actual
resolved addresses. Production blocks loopback, link-local and unspecified
addresses, including IPv4-mapped IPv6. Private/unique-local collector addresses
work when the global private-egress block is disabled. No proxy environment or
redirect can change the destination.

Missing, wrong-type, initializing and unhealthy references use the next healthy
configured log output; when none is healthy, the log is dropped with the existing
drop counter. A referenced connection failure does not prevent Gateway startup.
Invalid namespace/name syntax fails process configuration. References use Kubernetes
namespace/name syntax and bounds, including for a disabled output or fallback.

Hot updates retain a working connection while the replacement is invalid,
unresolved or fails its initial empty OTLP Export. This initial check includes
collector authentication and TLS acceptance within a three-second deadline. A
valid pending connection retries automatically.
Removal stops the exporter and probe; process shutdown gives each pipeline up to
five seconds to drain before cancelling its outstanding exports and joining its
worker. A replacement already retiring is joined before the active pipeline. This is best-effort log
delivery, without durable replay or retry to a second sink after a send failure.

`edgion_link_sys_health{link="observability/collector"}` and its flip counter use
the LinkSys key. Health describes TCP reachability, not authentication, successful
TLS negotiation on every reconnect, or delivery acknowledgment. Inspect collector
records to verify end-to-end delivery. SDK transport diagnostics that can reveal
endpoints or certificate details remain suppressed even with `RUST_LOG=trace`.

Only OTLP/gRPC log export is supported. There is no `protocol` field or legacy
inline OTLP output shape; OTLP/HTTP, trace export and application metric export are
outside this connector.
