# Stream Plugins

> **🔌 Edgion extension**
> 
> Stream plugins are connection-level (L4) plugins provided by the `EdgionStreamPlugins` CRD; they run before HTTP parsing and are not part of standard Gateway API.

Stream plugins apply access control to raw TCP/TLS/UDP connections at Pingora's
ConnectionFilter stage. They decide whether a connection (or, for UDP, a
session) is allowed before any payload is forwarded.

## How attachment works

Stream plugins are **not** inline in the route. You define an
`EdgionStreamPlugins` resource and reference it from the route with the
`edgion.io/edgion-stream-plugins` annotation. The annotation value is
`namespace/name` (or just `name` when the plugin lives in the same namespace as
the route).

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-deny"
```

The same annotation works on `TCPRoute`, `TLSRoute`, `UDPRoute`, and at the
Gateway listener level. On `UDPRoute` only the Stage-1 ConnectionFilter
(`spec.plugins`) runs, because UDP has no TLS handshake.

## Supported plugins

| Type | Config object | Description |
|------|---------------|-------------|
| `IpRestriction` | inline rules | IP allow/deny groups embedded directly in the plugin. |
| `GlobalConnectionIpRestriction` | `pluginMetadataRef` | References a `GlobalConnectionIpRestriction` PluginMetaData; Center-managed and synced across Controllers. |
| `ConnectionRateLimit` | `config` object | Redis-backed per-source-IP connection rate limiting at the pre-TLS accept path. See the [ConnectionRateLimit section](#connectionratelimit-pre-tls-connection-rate-limiting) below. |

Each entry under `spec.plugins` has an optional `enable` flag (default `true`),
a `type`, and a `config` object.

### `IpRestriction` config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allow` | Array\<Object\> | none | Allow groups; each is `{name, cidrs}`. A non-empty `allow` turns on whitelist mode. |
| `deny` | Array\<Object\> | none | Deny groups; each is `{name, cidrs}`. Evaluated before `allow`. |
| `defaultAction` | Enum | `allow` | Action when no rule matches. Allowed values: `allow` \| `deny`. |

At least one of `allow` or `deny` must be non-empty.

#### Group sub-fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | **Required** | Group name, unique within the plugin (admin label only). |
| `cidrs` | Array\<String\> | **Required** | CIDR strings or bare IPs (bare IPs become `/32` or `/128`). |
| `description` | String | none | Optional human-readable note. |

Evaluation order: `deny` first, then `allow`, then `defaultAction`. For raw TCP
a denied connection is simply closed (no protocol-level response exists at L4).

### `GlobalConnectionIpRestriction` config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `pluginMetadataRef.name` | String | **Required** | Name of the `GlobalConnectionIpRestriction` PluginMetaData. |
| `pluginMetadataRef.namespace` | String | parent ESP namespace | Namespace of the PluginMetaData; inherited from the parent `EdgionStreamPlugins` if omitted. |

The referenced PluginMetaData stores the rules centrally under
`spec.metadata.config`, with a kill switch (`enable`), a selected `activeProfile`,
and named `profiles` so you can swap rule sets instantly. This lets multiple
Controllers share one policy.

## Quick start (inline IpRestriction)

### Step 1: Define the plugin

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-ip-restriction-allow
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: allowed-clients
            cidrs:
              - "10.0.0.0/8"          # internal network only
        defaultAction: deny           # reject everything not allowed
```

### Step 2: Reference it from the route

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-allow"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

### Step 3: Apply

```bash
kubectl apply -f stream-plugins.yaml
kubectl apply -f tcp-route.yaml
```

Only clients in `10.0.0.0/8` can now reach the Redis service.

## Center-managed restriction (GlobalConnectionIpRestriction)

Store the rules once in a `GlobalConnectionIpRestriction` PluginMetaData (which
supports named `profiles` and a kill switch) and reference it from a stream
plugin. This is the form Center syncs to multiple Controllers.

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: gcir-policy
  namespace: edgion-test
spec:
  metadata:
    type: GlobalConnectionIpRestriction
    config:
      enable: true                    # kill switch; false disables the policy
      activeProfile: main             # which profile is live
      profiles:
        main:
          defaultAction: deny
          allow:
            - name: office
              cidrs:
                - "10.0.0.0/8"
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-stream
  namespace: edgion-test
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        pluginMetadataRef:
          name: gcir-policy
          # namespace omitted: inherits parent ESP namespace (edgion-test)
---
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: gcir-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/gcir-stream"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## Cross-namespace reference

Point at a plugin in another namespace using `namespace/name`:

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-route
  namespace: app
  annotations:
    edgion.io/edgion-stream-plugins: "security/common-policy"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## Edge cases

### Same annotation on a UDPRoute

`UDPRoute` accepts the identical annotation. Only Stage-1 (`spec.plugins`) runs;
the IP check is evaluated on the first datagram of each session (fail-close), so
a denied client never establishes a session.

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-route-with-plugins
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/udp-ip-restriction-deny"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: udp-plugins
  rules:
    - backendRefs:
        - name: udp-echo-backend
          port: 9300
```

### Deny a single host inside an allowed range

`deny` is evaluated before `allow`, so you can permit a broad CIDR while carving
out one address. Group names are arbitrary labels used only for organization.

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: carve-out
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
        deny:
          - name: quarantine
            cidrs:
              - "10.6.6.6/32"        # blocked even though it is inside 10.0.0.0/8
        defaultAction: deny
```

## ConnectionRateLimit: pre-TLS connection rate limiting

`ConnectionRateLimit` bounds the rate of new inbound connections per source IP.
The Redis check runs **before** the TLS handshake (Pingora's `ConnectionFilter`
stage), so it can shed connections before any TLS resources are allocated.

### Why `redisTimeout` is hard-capped at 1 s

The rate-limit check runs on the inbound pre-TLS accept path and holds the
accepted file descriptor — and the Pingora task that owns it — open for the
duration of the Redis round-trip. A Redis **connection pool does not remove
this backpressure**: the pool is *outbound* (Gateway → Redis); the pileup is
*inbound* (client fds held while awaiting a Redis response). During a Redis
brownout, every new inbound connection stalls for up to `redisTimeout` before
`onRedisFailure` takes over. The 1 s hard cap limits how long stalled inbound
fds can accumulate.

> **`perSourceIp` has no local backstop.** Unlike the `perListener` dimension
> (which has a local token bucket that stays live even when Redis is
> unreachable), `perSourceIp` has no in-process fallback — every accepted
> connection must reach Redis. Set `onRedisFailure` explicitly if you need
> fail-close behavior.

### Config fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `redisRef` | String | **Required** | Redis LinkSys reference in `"namespace/name"` format. |
| `algorithm` | Enum | `SlidingWindow` | Rate-limiting algorithm: `SlidingWindow` \| `FixedWindow` \| `TokenBucket`. |
| `redisTimeout` | String | `"100ms"` | Wall-clock deadline for the accept-path Redis call. Hard-capped at `1s`. When exceeded, `onRedisFailure` applies. |
| `perListener` | Object | none | Per-listener rate limit with a local token-bucket backstop (available even when Redis is unreachable). |
| `perSourceIp` | Object | none | Per-source-IP rate limit. Requires Redis — no local backstop. See caution above. |

#### `perSourceIp` / `perListener` sub-fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `rate` | Integer | **Required** | Maximum new connections per interval. |
| `interval` | String | **Required** | Time window, e.g. `"1s"`, `"1m"`. |
| `redisDistributed` | Boolean | `false` | (`perListener` only) When `true`, also enforces the cap fleet-wide via Redis in addition to the local gate. |
| `onRedisFailure` | Enum | `Allow` (`perSourceIp`) / `Deny` (`perListener`) | Behavior when Redis is unavailable or `redisTimeout` elapses. `Allow` = fail-open; `Deny` = fail-close. |

### Observability

The metric `edgion_connection_rate_limit_redis_failure_total` with labels
`dimension` (`listener` or `source_ip`) and `cause` (`unavailable` or
`runtime`) increments on every L4 Redis failure, regardless of the
`onRedisFailure` setting. This keeps fail-open behavior visible in Prometheus.

PromQL alert example:

```promql
rate(edgion_connection_rate_limit_redis_failure_total{dimension="source_ip"}[5m]) > 0
# L4 limiter silently failing open
```

### Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: conn-rate-limit
  namespace: edgion-test
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        redisRef: "default/shared-redis"
        algorithm: SlidingWindow
        redisTimeout: "200ms"     # default 100ms; hard-capped at 1s
        perListener:
          rate: 10000
          interval: "1s"
          redisDistributed: false
          # onRedisFailure: Deny  # default for perListener
        perSourceIp:
          rate: 50
          interval: "1s"
          # onRedisFailure: Allow # default for perSourceIp
```

Attach to a route as with any stream plugin:

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: rate-limited-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/conn-rate-limit"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-app
  rules:
    - backendRefs:
        - name: app-service
          port: 8080
```

## Troubleshooting

### Plugin not taking effect

1. Confirm the plugin resource exists:
   ```bash
   kubectl get edgionstreamplugins -A
   ```
2. Check the annotation value (must be `name` or `namespace/name`):
   ```bash
   kubectl get tcproute <name> -o yaml | grep -A2 annotations
   ```
3. Check the Gateway logs:
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```

### Connection refused unexpectedly

`deny` wins over `allow`, and `defaultAction: deny` rejects anything not listed.
Verify your client IP falls inside an `allow` group's `cidrs` and is not caught
by a `deny` group.
