# Stream Plugins

> **🔌 Edgion extension**
> 
> Stream plugins are connection-level (L4) plugins provided by the `EdgionStreamPlugins` CRD; they run before HTTP parsing and are not part of standard Gateway API.

Stream plugins apply access control to raw TCP/TLS connections or UDP sessions
before any payload is forwarded. Gateway-level attachments on Pingora `Service`
listeners run at the `ConnectionFilter` stage. UDPRoute attachments instead run
in the UDP proxy when a new source-address session is admitted.

## GatewayClass execution policy

`EdgionGatewayConfig.spec.pluginPolicy` controls whether named `stream/` and
`tlsRoute/` plugins may execute for a bound GatewayClass. `block` denies new
applicable connections or sessions; `bypass` skips the entry. Policy edits apply
to new admissions without restarting listeners. Established TCP/TLS connections
and UDP sessions keep their admitted behavior.

Before routing on a shared listener, every serving Gateway must have an
equivalent connection policy view; incompatible views deny new connections.
Route admission then uses the matched Gateway's class. See the
[plugin-policy guide](../../ops-guide/plugin-policy.md) for configuration and
missing/invalid-policy behavior.

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
Gateway listener level, but attachment lifecycle differs by protocol. On
`UDPRoute`, only the connection-context runtime (`spec.plugins`) runs, once when
a new UDP session is admitted, because UDP has no TLS handshake. A Gateway-level
attachment does not cover a UDP listener because UDP listeners do not use
Pingora's `Service` or listener `ConnectionFilter`; attach the resource directly
to `UDPRoute` instead. TLSRoute executes only `spec.tlsRoutePlugins`; a nonempty
Stage-1-only resource fails closed. See the [TLSRoute stage contract](../../dev-guide/stream-plugin-development.md#tlsroute-stage-contract).

## Supported plugins

| Type | Config object | Description |
|------|---------------|-------------|
| `IpRestriction` | inline rules | IP allow/deny groups embedded directly in the plugin. |
| `GlobalConnectionIpRestriction` | inline config + optional `activeProfileRef` | Full IP rule set (named `profiles`) inline in the plugin; an optional `Selector` EdgionConfigData hot-swaps **which profile is active**. Center-managed and synced across Controllers. |
| `ConnectionRateLimit` | `config` object | Local per-listener protection with optional Redis shared limits. Gateway attachments run before TLS; UDPRoute attachments apply to new-session admission. See the [ConnectionRateLimit section](#connectionratelimit-connection-and-udp-session-rate-limiting) below. |

Each entry under `spec.plugins` has an optional `enable` flag (default `true`),
a `type`, and a `config` object.

### `IpRestriction` config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allow` | Array\<Object\> | none | Allow groups; each is `{name, cidrs}`. A non-empty `allow` turns on whitelist mode. |
| `deny` | Array\<Object\> | none | Deny groups; each is `{name, cidrs}`. Evaluated before `allow`. |
| `allowRefs` | Array\<Object\> | none | References to central `IpList` EdgionConfigData whose CIDRs merge into the allowlist. Each ref is `{name, namespace?}`; `namespace` defaults to the parent `EdgionStreamPlugins` namespace. Configuring any `allowRefs` puts the plugin in whitelist mode. A missing or disabled IpList fails closed (deny-all) and is reported via `ResolvedRefs=False`. |
| `denyRefs` | Array\<Object\> | none | Same as `allowRefs` but merges into the denylist. A missing or disabled IpList under-blocks (still reported via `ResolvedRefs=False`). |
| `defaultAction` | Enum | `allow` | Action when no rule matches. Allowed values: `allow` \| `deny`. |

At least one of `allow`/`deny` or `allowRefs`/`denyRefs` must be non-empty.

#### Group sub-fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | **Required** | Group name, unique within the plugin (admin label only). |
| `cidrs` | Array\<String\> | **Required** | CIDR strings or bare IPs (bare IPs become `/32` or `/128`). |
| `description` | String | none | Optional human-readable note. |

Evaluation order: `deny` first, then `allow`, then `defaultAction`. For raw TCP
a denied connection is simply closed (no protocol-level response exists at L4).

### `GlobalConnectionIpRestriction` config

This plugin carries its **complete** IP rule set inline (git-owned). The only
hot-swappable input is *which profile is active*, supplied by an optional
`Selector` EdgionConfigData referenced via `activeProfileRef`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable` | Boolean | **Required** | Kill switch. `false` disables the plugin (all connections allowed). |
| `activeProfile` | String | **Required** | Default active profile name; must exist in `profiles`. |
| `profiles` | Map\<String, Object\> | **Required** | Named rule sets. Each profile is `{ defaultAction, allow?, deny?, allowRefs?, denyRefs? }` (same group shape as `IpRestriction`). Each profile may also reference central IpLists via per-profile `allowRefs`/`denyRefs` (distinct from `activeProfileRef`, which selects which profile is active, not an IpList). |
| `description` | String | none | Optional human-readable note. |
| `activeProfileRef.name` | String | none | Optional. Name of a `Selector` EdgionConfigData that overrides which profile is active. |
| `activeProfileRef.namespace` | String | parent ESP namespace | Namespace of the Selector; inherited from the parent `EdgionStreamPlugins` if omitted. |

The referenced `Selector` carries only `active: <profileName>` (plus an optional
`description`). At connection time the effective profile is the selector's `active`
when it names one of the plugin's `profiles`; otherwise — selector absent, disabled,
wrong type, `active: null`, or an unknown profile name — the plugin falls back to the
inline `activeProfile` (**fail-to-base**). A selector can only switch among the
git-owned profiles; it can never inject rules or disable the plugin.

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

## Center-managed active profile (GlobalConnectionIpRestriction)

Author the **full** rule set once, inline in the plugin (git-owned, reviewed). Then
let Center sync a small `Selector` EdgionConfigData that picks **which profile is
active** across multiple Controllers — without ever shipping the rules themselves.

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-stream
  namespace: edgion-test
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        enable: true                    # kill switch (false disables the plugin)
        activeProfile: locked           # git-default profile (safe)
        profiles:
          locked:
            defaultAction: deny
            allow:
              - name: office
                cidrs: ["10.0.0.0/8"]
          open:
            defaultAction: deny
            allow:
              - name: any
                cidrs: ["0.0.0.0/0"]
        activeProfileRef:               # optional: hot-swap the active profile
          name: gcir-active
          # namespace omitted: inherits parent ESP namespace (edgion-test)
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: gcir-active
  namespace: edgion-test
spec:
  enable: true
  data:
    type: Selector
    config:
      active: open                      # switch to "open"; null/absent → base default (locked)
      description: "temporarily widen access"
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

To widen or tighten access you switch `active` to a different **pre-authored**
profile; the selector cannot inject new rules or open everything — that must be a
reviewed profile in git. If the Selector is removed or names an unknown profile,
the plugin falls back to the inline `activeProfile` (`locked`).

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

## ConnectionRateLimit: connection and UDP session rate limiting

`ConnectionRateLimit` supports local per-listener protection combined with
Redis-backed shared limits. When attached to a Gateway listener, checks run
**before** the TLS handshake. TCPRoute attachments run after route matching.
UDPRoute attachments run when the first datagram attempts to create a new
source-address session. They do not count packets on established sessions, and
a Gateway-level attachment does not cover a UDP listener.

### Choosing local and shared limits

For routine connection burst protection, configure `perListener` with
`redisDistributed: false`. This uses a local token bucket without Redis.
When shared limits are needed, the same plugin can combine the local gate with
Redis: evaluation proceeds through the local `perListener` gate, optional
distributed `perListener` check, then optional `perSourceIp` check. A denial
stops evaluation. Setting `redisDistributed: true` keeps the local gate enabled.

The combined example below uses local `perListener` protection plus Redis
`perSourceIp` limits. Redis connection limiting remains an opt-in capability;
size it against the expected new-connection or UDP session-admission rate and
dependency latency.
For HTTP business quotas, prefer request-level limits: connection counts do
not represent request counts on persistent or multiplexed connections.

### Why `redisTimeout` is hard-capped at 1 s

For Gateway listener attachments, each Pingora accept task holds one accepted
socket while awaiting the filter. With Pingora's default one accept task per
listening FD, later connections queue in the kernel backlog during a Redis
wait. This bounds sockets undergoing filtering per accept task, but can reduce
new-connection throughput. A Redis connection pool or a higher process FD limit
does not remove this serialization. Existing established connections are not
directly blocked by this accept-loop wait.

`redisTimeout` defaults to `100ms` and is capped at `1s` **per Redis dimension**;
multiple dimensions or plugin entries can add their waits. Missing or unhealthy
Redis clients apply the failure policy immediately. Otherwise even fail-open
may wait until the call times out. Local filtering reduces Redis load but does
not remove the wait for connections that pass it. Pingora's
`listener_tasks_per_fd` controls accept parallelism; it is distinct from the
process FD limit and is not a `ConnectionRateLimit` field. Increasing parallelism
also increases concurrent Redis checks and needs workload validation. The
handshake timeout starts after filtering and does not bound this wait.

> **`perSourceIp` has no local backstop.** Unlike the `perListener` dimension
> (which has a local token bucket that stays live even when Redis is
> unreachable), `perSourceIp` has no in-process fallback. Connections or UDP
> session-admission attempts reaching this dimension require a Redis decision or
> the configured failure policy.
> Set `onRedisFailure` explicitly if you need
> fail-close behavior.

### Config fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `redisRef` | String | none | Required for `perSourceIp` or distributed `perListener`; Redis LinkSys reference in `"namespace/name"` format. |
| `algorithm` | Enum | `SlidingWindow` | Rate-limiting algorithm: `SlidingWindow` \| `FixedWindow` \| `TokenBucket`. |
| `redisTimeout` | String | `"100ms"` | Deadline per Redis dimension, capped at `1s`. When exceeded, `onRedisFailure` applies. |
| `perListener` | Object | none | Per-listener rate limit with a local token-bucket backstop (available even when Redis is unreachable). |
| `perSourceIp` | Object | none | Per-source-IP rate limit. Requires Redis — no local backstop. See caution above. |

**Bucket scope**: counters — both the Redis keys and the local token bucket — are namespaced by the plugin entry that owns them, `<namespace>/<name>/#<index>` of the EdgionStreamPlugins resource and the entry's position in `plugins`. Two `ConnectionRateLimit` entries can be active on one listener port (a listener-level plus a TCPRoute-level attachment, or two entries in one resource); without this scope they would share one counter while each enforced its own `rate`, and with differing `interval` would corrupt each other's window. Every Gateway instance derives the same scope from the same resource, so cluster-wide counting is unaffected, and the scope is stable across hot reload. Reordering entries changes their identity: an entry moving to a previously used position may inherit that position's retained local budget. Keep names and positions stable when budget continuity matters.

**Local budget on reload**: each listener port adopts an updated limit when a
connection or UDP session-admission attempt first reaches that entry with the
new configuration. Elapsed time is
credited at the previous rate before the change. Increasing `rate` adds only the
capacity difference; decreasing it caps the remaining balance. Changing only
Redis settings, including `keyPrefix`, does not reset the local budget. The same
entry and port retain their local state through disable/delete and re-enable/recreate
within one Gateway process; idle time accrues at the last activated local rate.

An admission already using an older configuration may finish during reload. Once
the port adopts the new local limit, such an admission consumes the updated local
budget but keeps its original Redis settings. These are sequential checks, not
one atomic local-and-Redis transaction. A later Redis denial does not refund the
local token. Repeating the same resource at both listener and TCPRoute levels
still evaluates and consumes that entry at both levels.

#### `perSourceIp` / `perListener` sub-fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `rate` | Integer | **Required** | Maximum new TCP/TLS connections or UDP session-admission attempts per interval. Established UDP session packets are not counted. |
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
