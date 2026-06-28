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
| basic (Redis / Etcd / ES) | `kubernetes.io/basic-auth` or `Opaque` | `password` | `username` |
| apiKey (Elasticsearch) | `Opaque` | `apiKeyId`, `apiKeySecret` | — |
| bearer (Elasticsearch) | `Opaque` | `bearerToken` | — |
| TLS (all backends) | `kubernetes.io/tls` or `Opaque` | At least one of `tls.crt` / `tls.key` / `ca.crt` | — |

Application-layer key names use camelCase; TLS keys follow the Kubernetes `kubernetes.io/tls` standard.

> **Webhook credentials are different.** The webhook connector no longer has a typed `auth` block. Instead, you inject any credential as a custom request header (or arg/cookie) that reads from a Secret. The Secret key name is **operator-chosen** (whatever you put in `key:`), and an optional `prefix:` lets you prepend `Bearer ` / `Basic ` / etc. See [Webhook connector → Custom field sources](#custom-field-sources).

### TLS

Every backend accepts the same `tls` block for securing the connection.

```yaml
tls:
  enabled: true
  verify: true                 # verify peer certificate (default true); false is honored (operator responsibility)
  caSource: SecretRef          # System | SecretRef | Inline
  caSecretRef:
    name: my-ca-bundle         # Secret holding ca.crt (used when caSource: SecretRef)
  # caCert: |                  # Inline PEM CA bundle (used when caSource: Inline)
  #   -----BEGIN CERTIFICATE-----
  #   <base64-encoded-cert>
  #   -----END CERTIFICATE-----
  clientSecretRef:
    name: my-client-tls        # Secret holding tls.crt + tls.key for client mTLS
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | Boolean | `false` | Enable TLS for the connection. For URLs that already imply TLS (`https://`, `rediss://`), this only adds custom CA / client-certificate handling. |
| `verify` | Boolean | `true` | Verify the peer TLS certificate. `verify: false` is honored (operator responsibility); a WARN is logged at startup. Redis and Etcd always verify (`verify: false` is rejected at startup — the SDK has no skip-verify API). |
| `caSource` | Enum | — | Trust-anchor source: `System` (built-in webpki roots), `SecretRef` (private CA from `caSecretRef`), or `Inline` (PEM in `caCert`). A custom CA **replaces** the public roots. |
| `caSecretRef` | Object | none | Kubernetes Secret holding a private CA bundle (`ca.crt` key). Required when `caSource: SecretRef`. |
| `caCert` | String | none | Inline PEM CA bundle. Used when `caSource: Inline`. |
| `clientSecretRef` | Object | none | Kubernetes Secret holding the client mTLS identity (`tls.crt` / `tls.key` keys). |

The client mTLS identity (certificate + private key) must be provided via `clientSecretRef`.

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
      clientSecretRef:
        name: etcd-client-tls
    namespace: "/my-app/"
```

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoints` | Array\<String\> | **Required** | Etcd endpoints; at least one. Example: `"https://etcd-1:2379"`. |
| `auth` | Object | none | Authentication. The Secret typically provides both `username` and `password`. |
| `tls` | Object | none | TLS configuration; see [TLS](#tls). Etcd (like Redis) rejects `verify: false` in any environment. |
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

A webhook connector lets plugins (ctx-set, key-auth, hmac-auth, and so on) call an external HTTP service for user resolution, permission checks, context enrichment, audit logging, and similar lookups. The connector is the **single source of truth** for the outbound request: it declares the backend (`target.backendRef`), and a `request` block that defines exactly which path, method, query args, headers, and cookies are sent. Consuming plugins can no longer silently reshape the request — they can only feed values into the named slots the webhook author declared, optionally override the path (when allowed), and parse the response. Outbound fields follow an **allow-list** model: nothing is forwarded unless the webhook explicitly names it, so a misbehaving upstream plugin cannot leak headers the operator never intended to expose.

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
      backendRef:
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
    successStatusCodes: [200]
    maxResponseBytes: 2048
```

### Target

Every webhook references its backend through a single `backendRef`, the same `BackendObjectReference` shape that routes already use. There is no longer a `type: uri` / `type: service` choice — `backendRef` is the only target form.

```yaml
target:
  backendRef:
    group: ""              # optional; defaults to the core API group
    kind: Service          # Service | ServiceClusterIp | ServiceEndpointSlice (empty defaults to Service)
    name: webhook-backend
    namespace: production  # optional; defaults to the LinkSys namespace
    port: 8080             # required
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target.backendRef.group` | String | core (`""`) | API group of the referenced object. |
| `target.backendRef.kind` | String | `Service` | Target kind. Allowed: `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, or empty (defaults to `Service`). |
| `target.backendRef.name` | String | **Required** | Kubernetes Service name. |
| `target.backendRef.namespace` | String | LinkSys namespace | Service namespace. |
| `target.backendRef.port` | Integer | **Required** | Service port; must be greater than 0. A missing port is rejected at admission with `Accepted=False`. |

Resolution by kind:

- `Service` (default, and empty/unset): the gateway resolves ready endpoints from the Kubernetes EndpointSlice store at call time.
- `ServiceEndpointSlice`: explicit form of the same EndpointSlice resolution.
- `ServiceClusterIp`: resolved via the Service ClusterIP DNS name (kube-proxy).

`kind: ExternalName` and `ServiceImport` are **not** supported as webhook targets and are rejected at admission with a descriptive `Accepted=False` message. To reach an external endpoint, create a proxy Service (for example a `LoadBalancer` Service, or a `ClusterIP` Service fronted by an ingress controller) that routes to the external host, then reference that Service by name. See [Migration from the old schema](#migration-from-the-old-schema).

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
| `required` | The webhook call is treated as failed. `statusOnError` / `allowDegradation` then apply exactly as for any other webhook failure. | Fields the backend cannot function without. |

`presence` and `allowDegradation` are orthogonal and compose: `presence: required` decides whether a missing field *counts as a failure*; `allowDegradation` then decides whether that failure aborts the plugin chain (`false`, default) or silently skips the webhook and continues (`true`).

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
| `${clientIp}` | Real client IP. |

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
| `tls` | Object | none | TLS configuration; see [TLS](#tls). Defaults to the system CA bundle when unset. |
| `successStatusCodes` | Array\<Integer\> | any 2xx | HTTP status codes treated as success. |
| `maxResponseBytes` | Integer | `1024` | Maximum response body size to read in bytes. Clamped at call time to `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes` (default 16 KiB). |

> **Removed fields.** `requestMethod`, `auth`, and `defaultHeaders` are deprecated. They are still accepted during a rolling upgrade but ignored at runtime, and the controller logs a WARN when it sees them. They will be removed in the next release — migrate before then. See [Migration from the old schema](#migration-from-the-old-schema).

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
      backendRef:
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
      backendRef:
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
    target:
      backendRef: { kind: Service, name: authz, port: 8080 }
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
    target:
      backendRef: { kind: Service, name: audit-sink, port: 8080 }
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
    target:
      backendRef: { kind: Service, name: key-service, port: 8080 }
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
    target:
      backendRef: { kind: Service, name: enrich, port: 8080 }
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
            template: "${clientIp}"
```

#### 6. Presence policies (required / skipIfMissing / warnIfMissing)

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: authz, port: 8080 }
    allowDegradation: false        # required-field failure surfaces statusOnError
    statusOnError: 503
    request:
      headers:
        origin:
          - name: X-Trace-Id                       # default: skipIfMissing
          - name: X-User-Id
            presence: required                     # missing → call fails → 503
          - name: X-Org-Id
            presence: warnIfMissing                # missing → PluginLog alert, continues
```

#### 7. Path override (allowOverride)

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: user-resolver, port: 8080 }
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
    target:
      backendRef: { kind: Service, name: user-service, port: 8080 }
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
# Consuming plugin (ctx-set) on an HTTPRoute filter
type: ctx-set
vars:
  - { name: user_id, template: "${header:X-User-Id}" }
  - { name: action, value: "get-user" }
  - name: resolved_user
    from:
      type: webhook
      webhookRef: production/user-resolver
      pathOverride: "/special/path"   # honored only because allowOverride: true
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
      backendRef:
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

### Migration from the old schema

The webhook schema changed in this release. Old fields are accepted during a rolling upgrade but ignored at runtime (the controller logs a WARN), and will be removed in the next release. Migrate every webhook LinkSys before then.

| Old | New |
|-----|-----|
| `target.type: uri` + `target.uri` | `target.backendRef` to a (proxy) Service — raw URLs are no longer supported |
| `target.type: service` + `target.name/port` | `target.backendRef.{kind,name,namespace,port}` (drop the `type` tag) |
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
      backendRef:
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

Consumer-side, `pathSuffix` and `forwardHeaders` are removed:

```yaml
# Old
- type: key-auth
  config:
    keys:
      - type: webhook
        webhookRef: production/user-resolver
        pathSuffix: "/special/path"
        forwardHeaders: ["X-Tenant-Id", "X-User-Id"]

# New — declare path/headers on the webhook resource; consumer only overrides the path
- type: key-auth
  config:
    keys:
      - type: webhook
        webhookRef: production/user-resolver
        pathOverride: "/special/path"   # honored only if allowOverride: true on the webhook
```

> **External endpoints.** `target.type: uri` and `kind: ExternalName` are not supported. Front the external host with a proxy Service (LoadBalancer, or ClusterIP behind an ingress controller) and reference that Service by name.

### Webhook troubleshooting

**Webhook not resolving / "webhook not found".**
Verify the `target.backendRef`: the named Service must exist in the resolved namespace and the `port` must be set and valid. A LinkSys with `Accepted=False` is not loaded into the gateway runtime, so any consumer calling it hits the same "webhook unavailable" path. Run `kubectl get linksys <name> -o yaml` and read the `Accepted` condition message.

**`Accepted=False` on a `kind` value.**
Only `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, and empty are valid. `ExternalName` and `ServiceImport` are rejected — use a proxy Service instead.

**Required-field error / unexpected `statusOnError`.**
An `origin` field with `presence: required` was missing from the incoming request, which counts as a webhook failure. Confirm the client actually sends the header/arg/cookie (mind case: headers are case-insensitive, args and cookies are case-sensitive). To degrade instead of failing, set `allowDegradation: true`; to merely log, use `presence: warnIfMissing`.

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

The CA bundle is inlined (`caSource: Inline`). The client certificate and private key come from a Secret (`clientSecretRef`).

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
      caSource: Inline
      caCert: |
        -----BEGIN CERTIFICATE-----
        <base64-encoded-cert>
        -----END CERTIFICATE-----
      clientSecretRef:
        name: etcd-client-tls
```

## FAQ

### Q1: Why does my LinkSys report `Accepted=False/Invalid`?

**A**: The structure or per-backend validation failed. Common causes: an empty `endpoints` list, `topology.mode=sentinel` without a `sentinel` block (or `mode=cluster` without `cluster`), a webhook `target.backendRef` with a missing/zero `port` or an unsupported `kind` (only `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, empty are allowed), or a webhook `request` with conflicting fields (the same name in both `origin` and `custom`, a duplicate name within a list, or both `headers.origin[name: Cookie]` and a `cookies.*` list). The status `message` concatenates the specific errors.

### Q2: My credentials look correct, but `ResolvedRefs` is `False`. Why?

**A**: The Secret could not be resolved. Check the reason:

- `SecretNotFound` — the Secret does not exist yet. The Controller automatically requeues when the Secret is created.
- `RefNotPermitted` — a cross-namespace reference without a matching `ReferenceGrant`.
- `InvalidSecretType` — the `Secret.type` is not in the resolver's allowlist for that auth type.
- `MissingKeys` — the Secret is missing required keys (for example basic auth missing `password`, or apiKey missing `apiKeyId` / `apiKeySecret`).

### Q3: Can I put the password directly in the LinkSys YAML?

**A**: No. The auth and TLS structs enable `deny_unknown_fields`, so inline sensitive fields (`username`, `password`, `bearerToken`, `apiKeyId`, `hmacKey`, `clientKey`, ...) cause parsing to fail rather than being ignored. Always use a `secretRef`.

### Q4: Can I disable TLS verification (`verify: false`) for Redis or Etcd?

**A**: No. Redis and Etcd clients do not support disabling certificate verification — `verify: false` is rejected at startup for these connectors in any environment. This is intentional: to connect to a backend with a private CA, use `tls.caSource: Inline` + `tls.caCert` (inline PEM) or `tls.caSource: SecretRef` + `tls.caSecretRef` instead.

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
