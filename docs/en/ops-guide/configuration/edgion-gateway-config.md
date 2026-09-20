# EdgionGatewayConfig

EdgionGatewayConfig is a cluster-scoped CRD (`edgion.io/v1alpha1`) that configures behavior at the GatewayClass level. It is referenced via `GatewayClass.spec.parametersRef`. Most request- and connection-scoped policies update dynamically; fields used to construct an HTTP listener are explicitly marked as restart-bound.

## Difference from Gateway YAML

| Layer | Gateway YAML | EdgionGatewayConfig |
|-------|-------------|---------------------|
| Scope | Single Gateway process | Entire GatewayClass |
| How changes apply | Requires process restart | Field-specific; most policies are dynamic, while `spec.server` requires a process restart |
| Typical use | Process connection params, log paths, rate limit precision | Timeout policies, security rules, global plugins, real IP extraction |

**When to use Gateway YAML vs EdgionGatewayConfig:**
- Change the Controller gRPC address → Gateway YAML `gateway.server_addrs`
- Change Admin / Metrics ports → Gateway YAML (process startup layer)
- Configure HTTP timeouts, real IP, global plugins → EdgionGatewayConfig

## How to Associate

Reference EdgionGatewayConfig from a GatewayClass via `parametersRef`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config  # Matches metadata.name of the EdgionGatewayConfig below
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config   # Cluster-scoped — no namespace needed
spec:
  server:
    enableCompression: false
    downstreamKeepaliveRequestLimit: 1000

  httpTimeout:
    client:
      readTimeout: "60s"
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"

  maxRetries: 3

  realIp:
    trustedIps:
      - name: private-ranges
        cidrs: ["10.0.0.0/8", "172.16.0.0/12"]
    realIpHeader: "X-Forwarded-For"
    recursive: true

  securityProtect:
    xForwardedForLimit: 200
    requireSniHostMatch: true
    fallbackSni: ""
    tlsProxyLogRecord: true
    rejectDuplicateHost: true

  requestBody:
    enabled: true
    defaultMemoryBufferSize: "128KiB"
    # maxMemoryBufferSize: "1MiB"
    defaultMaxBodySize: "32MiB"
    # maxBodySize: "64MiB"
    storageOperationTimeout: "5s"

  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"

  accessLogExtern:
    unmaskedKeys:
      header: [user-agent, origin, content-type, content-length, accept, x-request-id]
      respHeader: [content-type, content-length, cache-control, retry-after, x-request-id]
      query: []
      cookie: []
      ctx: []

  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204

  linkSys:
    webhookMaxResponseBytes: 16384   # Hard cap for LinkSys webhook response body (16 KiB)
    maxInstancesPerKind: 200         # Max live+draining instances per provider kind
```

> Note: URI path normalization (`%2F` rejection, invalid percent-encoding rejection, etc.) is **not** part of EdgionGatewayConfig — it is configured in the Gateway YAML `path_normalization` section. See [Gateway Configuration](./gateway-config.md).

## Configuration Parameters

### spec.server

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enableCompression` | `Boolean` | `false` | Gzip eligible downstream responses with a valid `Content-Length` and no `Transfer-Encoding`; unknown-length and SSE responses remain uncompressed to preserve incremental delivery |
| `downstreamKeepaliveRequestLimit` | `Integer` | `1000` | Max requests per downstream connection; `0` means unlimited |

Both fields are captured when the HTTP listener is constructed. Updating them on an existing
Gateway process requires a restart. Pingora worker threads, shutdown periods, the upstream
keepalive pool, and the Pingora error-log path belong to the local Gateway YAML `server` section;
they are not GatewayClass fields.

### spec.httpTimeout.client

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `readTimeout` | `Duration` | `60s` | Timeout for reading a client request, including each request-body chunk on both HTTP/1 and HTTP/2. The timer resets on every received chunk, so a stalled upload is released while a progressing one is never limited in overall duration. Note: this also bounds gRPC client-streaming/bidi requests — raise it if clients legitimately idle longer between messages |
| `writeTimeout` | `Duration` | `60s` | Timeout for writing a response to the client |
| `keepaliveTimeout` | `Duration` | `75s` | HTTP keepalive idle timeout |

### spec.httpTimeout.backend

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `defaultConnectTimeout` | `Duration` | `5s` | Timeout for connecting to an upstream server |
| `defaultRequestTimeout` | `Duration` | `60s` | Total request timeout including retries |
| `defaultIdleTimeout` | `Duration` | `300s` | Connection pool idle timeout |

### spec.tcpTimeout

Timeouts for L4 relays — plain TCP, TLS-terminated, and TLS-passthrough. UDP has its own session model and is not affected.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `idleTimeout` | Duration string | `1h` | Closes a relay that makes no byte progress in either direction for this window. The clock resets on any completed read or advancing partial write, so active and slow-but-draining transfers are not severed. |
| `connectTimeout` | Duration string | `10s` | Upper bound for establishing the upstream TCP connection (the TCP handshake). In all three relay types the upstream connect is plain TCP, so this bounds the connect itself, not a TLS handshake. A backend that silently drops SYNs (firewall, not-yet-ready pod) fails fast instead of stranding the downstream connection until the OS TCP retransmit timeout (~2m) fires. |

**Secure by default:** If you omit the entire `tcpTimeout` section, the `1h` idle timeout and `10s` connect timeout still apply — every L4 relay is bounded without configuration.

**Dynamic updates:** Changing `tcpTimeout` does not require a listener restart. A
TCP, TLS-terminated, or TLS-passthrough connection copies the latest resolved
values when it enters L4 proxy processing. Existing connections keep the values
they started with, including across TLSRoute connect retries and the relay. A
GatewayClass `parametersRef` change or shared-listener ownership change remains a
listener-lifecycle change and is not applied dynamically.

**Opt-out and risk:** Set `idleTimeout: "0s"` (any zero-valued duration; a bare `"0"` is not valid GEP-2257 and is rejected at admission) to disable the idle timeout (unbounded relay). This re-opens a resource-exhaustion risk: a stalled or trickle-sending peer can hold a relay — and its socket, task, and buffers — open indefinitely, with no in-process connection cap. Only disable it if a higher layer bounds connection lifetime. Setting `connectTimeout: "0s"` likewise removes the application-level connect bound, falling back to the OS TCP retransmit timeout (~2m on Linux).

> **Known limitation:** An idle timeout does not stop a determined trickle attacker (a byte every N seconds resets the clock, on either the read or write half). Bounding that would require a total-duration cap, which would sever legitimate long-idle connections (DB pools, message queues, long-poll, SSH) and is not a sane default.

```yaml
spec:
  tcpTimeout:
    idleTimeout: 1h
    connectTimeout: 10s
```

### spec.maxRetries

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxRetries` | `Integer` | `3` | Maximum retry attempts on upstream connection failure |

> The older `httpTimeout.backend.defaultMaxRetries` field is **deprecated**. If present, its value is ignored and a warning is logged at config load; use the top-level `maxRetries` instead.

### spec.realIp

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trustedIps` | `Array<IpGroup>` | — (required when `realIp` is set) | List of trusted proxy IP groups; must be non-empty |
| `realIpHeader` | `String` | `X-Forwarded-For` | Header from which to extract the real client IP. Must be a valid HTTP header field name (RFC 9110 `token` charset) of at most 256 bytes |
| `recursive` | `Boolean` | `true` | Traverse the header right-to-left, skip trusted proxies, and take the first non-trusted IP |

Each `trustedIps` entry is an `IpGroup` object: `name` (required; ≤63 chars, charset `[a-zA-Z0-9._-]`, must start/end alphanumeric, unique within the list), `description` (optional), and `cidrs` (required; non-empty list of IP or CIDR strings).

Extraction logic: walk `realIpHeader` from right to left, skip addresses in any `trustedIps` group, and return the first non-trusted IP.

`spec.realIp` is validated at ingest with the same rules as the RealIp plugin. An empty `trustedIps`, a malformed or duplicated CIDR, an `IpGroup` name outside the label-value charset, or a `realIpHeader` that is not a valid HTTP header field name makes the resource report `Accepted=False` with `reason=Invalid`. On the Kubernetes path the API server rejects a malformed `realIpHeader` outright, since the CRD carries the header-name `pattern` and `maxLength`.

Controller preparse warns rather than refusing to publish, so the Gateway applies the same check as a backstop: when `spec.realIp` is invalid it logs an error and runs with real-IP extraction disabled, exactly as if `realIp` were unset. Note that `RealIpExtractor` itself is forgiving about individual CIDRs (it skips invalid entries with a warning), so a CIDR mistake that previously degraded silently now invalidates the whole `realIp` block.

### spec.securityProtect

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `xForwardedForLimit` | `Integer` | `200` | Maximum byte length of the `X-Forwarded-For` header; excess is truncated |
| `requireSniHostMatch` | `Boolean` | `true` | HTTPS listener-level 421 Misdirected Request detection; rejects requests where SNI does not match the Host header |
| `fallbackSni` | `String` | — | Fallback hostname used when the client provides no SNI |
| `tlsProxyLogRecord` | `Boolean` | `true` | Record TLS proxy connection logs |
| `allowLoopbackUpstream` | `Boolean` | `false` | Explicitly allow loopback upstream addresses for this GatewayClass. Link-local / metadata ranges remain blocked |
| `rejectDuplicateHost` | `Boolean` | `true` | Reject HTTP/1.1 requests carrying more than one `Host` header line with `400 Bad Request` (RFC 9110 §7.2 Host smuggling defense). Disable only as a transitional escape hatch for legacy middleware that emits duplicate `Host` lines |

### spec.requestBody

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | `Boolean` | `true` | Runtime capture switch. When disabled, body-aware plugins fail closed with the normal body-unavailable result |
| `defaultMemoryBufferSize` | `String` | `128KiB` | Default in-memory bytes before disk spill |
| `maxMemoryBufferSize` | `String` | unset | Effective business override for the memory spill threshold; falls back to `defaultMemoryBufferSize` |
| `defaultMaxBodySize` | `String` | `32MiB` | Default complete request-body cap |
| `maxBodySize` | `String` | unset | Effective business override for the body cap; falls back to `defaultMaxBodySize` |
| `storageOperationTimeout` | `String` | `5s` | Non-zero deadline for one disk-backed request-body operation, including waiting for storage-I/O admission and executing the file operation |

All sizes must be positive human-readable byte sizes. The effective memory
threshold must not exceed the effective body cap. Values are parsed when the
configuration update is published. Each new request copies the resolved policy
from an immutable snapshot without parsing sizes or taking a blocking lock. A
plugin's `body.maxBodySize` may further tighten the limit for that plugin.
`storageOperationTimeout` uses the shared GEP-2257 duration syntax (`500ms`,
`5s`, `1m`) and must be greater than zero. A storage deadline is reported as a
gateway-local storage failure, not as a downstream body-read timeout. Capture
failure disables HTTP/1 connection reuse when unread body bytes may remain;
HTTP/2 resets only the affected stream.

### spec.globalPluginsRef

Global plugins applied to all routes under this GatewayClass, executed before route-level plugins.

```yaml
globalPluginsRef:
  - name: "global-cors"       # EdgionPlugins resource name
    namespace: "edgion-system" # Defaults to "default"
```

### spec.accessLogExtern

The masking whitelist for [`AccessLogExtern`](../observability/access-log.md#extending-the-log-accesslogextern)-declared
access-log fields. Platform-owned — cluster-scoped, next to `globalPluginsRef`
— so a namespaced `EdgionPlugins` cannot widen masking for the whole fleet.

```yaml
accessLogExtern:
  unmaskedKeys:
    header:     [user-agent, origin, content-type, content-length, accept, x-request-id]
    respHeader: [content-type, content-length, cache-control, retry-after, x-request-id]
    query:      []
    cookie:     []
    ctx:        []
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `unmaskedKeys.header` | `[]String` | `[user-agent, origin, content-type, content-length, accept, x-request-id]` | Request header names logged in clear (case-insensitive) |
| `unmaskedKeys.respHeader` | `[]String` | `[content-type, content-length, cache-control, retry-after, x-request-id]` | Response header names logged in clear (case-insensitive) |
| `unmaskedKeys.query` | `[]String` | `[]` | Query parameter names logged in clear (case-sensitive) |
| `unmaskedKeys.cookie` | `[]String` | `[]` | Cookie names logged in clear (case-sensitive) |
| `unmaskedKeys.ctx` | `[]String` | `[]` | Ctx-var names logged in clear (case-sensitive) |

The whitelist is keyed by source type, not one flat list — a header named
`user-agent` is harmless, a cookie of the same name is not. `query`/`cookie`/`ctx`
default empty because their key names are entirely business-defined; there is
no name that is safe by default across every cluster. A miss still emits the
field, masked, rather than dropping it. `routeLabel`/`routeAnnotation` sources
are never masked and bypass this whitelist entirely. A hard blocklist (credential
headers, claim/payload ctx keys) cannot be named regardless of this whitelist.
Enforcement is two-part: a declaration naming a blocked key is **reported** on
the resource's `Accepted=False` status condition at `EdgionPlugins` ingest
(admission is lenient — the resource still lands in the cache conf_sync
serves to the gateway), and then actually **dropped at gateway load** by
`compile_plugins`'s field `retain()`, which discards any declared field whose
validation error is set before the spec is stored. Widening the blocklist
requires a code change, not a CR edit.

**Omitting a `unmaskedKeys` list keeps that list's default, it does not empty
it.** `UnmaskedKeys` fills each missing list from its own default, so
`unmaskedKeys: {ctx: [tenant_id]}` does not mean "only `tenant_id` is
unmasked" — it also keeps the default `header` and `respHeader` lists shown
above. To unmask only `tenant_id` and drop the header defaults, list every
category explicitly, e.g. `header: []`.

**Effective scope is per-listener, not per-Gateway**: when two Gateways of
different GatewayClasses share the same bind address/port, only the first
Gateway's `EdgionGatewayConfig` — including this whitelist — is bound for that
listener.

### spec.preflightPolicy

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | `String` | `cors-standard` | `cors-standard`: select OPTIONS requests with `Origin` + `ACRM`; `all-options`: select all OPTIONS requests for global handling. A route CORS filter still decides whether to respond locally. See [Preflight Policy](../gateway/preflight-policy.md). |
| `statusCode` | `Integer` | `204` | Final preflight response status (200-599) when no CORS plugin is attached |

### spec.linkSys

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `webhookMaxResponseBytes` | `Integer` | `16384` | Hard upper bound (bytes) for a LinkSys webhook response body. Each call's response is clamped to `min(per-resource maxResponseBytes, this value)`. Raise both values together when an external API legitimately returns a larger payload |
| `maxInstancesPerKind` | `Integer` | `200` | Max live plus draining persistent LinkSys instances per provider kind (1–10000). New keys are refused at the cap; updates to existing keys and deletes always proceed. Lowering the cap does not evict live instances |

Changes to `linkSys` take effect immediately through EdgionGatewayConfig partial-update — no Gateway restart required.

Because EdgionGatewayConfig is cluster-scoped, multiple instances may coexist. The Gateway derives a single process-global value deterministically across all instances rather than letting whichever was applied last win: `webhookMaxResponseBytes` and `maxInstancesPerKind` both use the **strictest** (`min`) value. When the last instance carrying `linkSys` is deleted, those globals reset to the defaults (`16384` / `200`). The same deterministic aggregation applies to `loadBalancing.degradeThreshold`, which uses the **highest** (`max`) value across instances and resets to `0` (degrade mode disabled) when none is configured.

### spec.loadBalancing

`degradeThreshold` is a percentage from 0 to 100, defaulting to 0 (disabled).
When the healthy endpoint fraction falls below the threshold, degrade mode bypasses
health/outlier filtering and balances across all endpoints. This can send traffic
to unhealthy backends. It applies only when health or outlier detection is active.

`EdgionBackendTrafficPolicy.spec.loadBalancer.degradeThreshold` overrides this
global default for its target Service, including an explicit 0 to disable degrade.
HTTP/gRPC requests capture the Service override on their first upstream attempt;
retries retain it across policy updates or removal. A request that captured no
override continues to use the current global default, even if a Service override
is subsequently added. Global-default updates remain independent and can affect
such requests. Endpoint membership and health remain live. TCP/UDP/TLS and request
mirrors use the current Service override or global default at each selection.

## Common Scenarios

### Configuring Real IP Extraction

When Gateway is deployed behind a load balancer, configure trusted proxy IPs to correctly extract the client's real IP:

```yaml
spec:
  realIp:
    trustedIps:
      - name: private-ranges
        cidrs:
          - "10.0.0.0/8"
          - "172.16.0.0/12"
          - "192.168.0.0/16"
    realIpHeader: "X-Forwarded-For"
    recursive: true
```

### Adjusting Timeout Policies

Relax timeouts for slow upstream services:

```yaml
spec:
  httpTimeout:
    client:
      readTimeout: "120s"
      writeTimeout: "120s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "10s"
      defaultRequestTimeout: "120s"
      defaultIdleTimeout: "300s"
```

### Attaching a Global CORS Plugin

```yaml
spec:
  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"
  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204
```

### Enabling Downstream Response Compression

```yaml
spec:
  server:
    enableCompression: true
```

### Disabling SNI Host Match Enforcement

Allow requests where the SNI does not match the Host header (useful in specific proxy topologies):

```yaml
spec:
  securityProtect:
    requireSniHostMatch: false
```

## Related Documentation

- [Controller Configuration](./controller-config.md)
- [Gateway Configuration](./gateway-config.md)
- [GatewayClass Configuration](../gateway/gateway-class.md)
- [Preflight Policy](../gateway/preflight-policy.md)
