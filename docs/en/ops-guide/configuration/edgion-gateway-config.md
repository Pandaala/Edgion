# EdgionGatewayConfig

EdgionGatewayConfig is a cluster-scoped CRD (`edgion.io/v1alpha1`) that configures runtime behavior at the GatewayClass level. It is referenced via `GatewayClass.spec.parametersRef` and takes effect dynamically without restarting the Gateway process.

## Difference from Gateway TOML

| Layer | Gateway TOML | EdgionGatewayConfig |
|-------|-------------|---------------------|
| Scope | Single Gateway process | Entire GatewayClass |
| How changes apply | Requires process restart | Dynamic (CRD update) |
| Typical use | Process connection params, log paths, rate limit precision | Timeout policies, security rules, global plugins, real IP extraction |

**When to use TOML vs EdgionGatewayConfig:**
- Change the Controller gRPC address → Gateway TOML `[gateway].server_addr`
- Change Admin / Metrics ports → Gateway TOML (process startup layer)
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
    threads: 0
    workStealing: true
    gracePeriodSeconds: 30
    gracefulShutdownTimeoutS: 10
    upstreamKeepalivePoolSize: 128
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
    trustedIps: ["10.0.0.0/8", "172.16.0.0/12"]
    realIpHeader: "X-Forwarded-For"
    recursive: true

  securityProtect:
    xForwardedForLimit: 200
    requireSniHostMatch: true
    fallbackSni: ""
    tlsProxyLogRecord: true

  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"

  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204

  enableReferenceGrantValidation: false
```

## Configuration Parameters

### spec.server

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `threads` | `Integer` | CPU core count | Pingora worker thread count; `0` means auto-detect |
| `workStealing` | `Boolean` | `true` | Enable Tokio work-stealing |
| `gracePeriodSeconds` | `Integer` | `30` | Seconds to wait for connections to drain during graceful shutdown |
| `gracefulShutdownTimeoutS` | `Integer` | `10` | Force-terminate after this many seconds during shutdown |
| `upstreamKeepalivePoolSize` | `Integer` | `128` | Upstream keepalive connection pool size |
| `errorLog` | `String` | — | Pingora internal error log file path |
| `enableCompression` | `Boolean` | `false` | Enable response compression for downstream clients |
| `downstreamKeepaliveRequestLimit` | `Integer` | `1000` | Max requests per downstream connection; `0` means unlimited |

### spec.httpTimeout.client

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `readTimeout` | `Duration` | `60s` | Timeout for reading a client request |
| `writeTimeout` | `Duration` | `60s` | Timeout for writing a response to the client |
| `keepaliveTimeout` | `Duration` | `75s` | HTTP keepalive idle timeout |

### spec.httpTimeout.backend

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `defaultConnectTimeout` | `Duration` | `5s` | Timeout for connecting to an upstream server |
| `defaultRequestTimeout` | `Duration` | `60s` | Total request timeout including retries |
| `defaultIdleTimeout` | `Duration` | `300s` | Connection pool idle timeout |

### spec.maxRetries

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxRetries` | `Integer` | `3` | Maximum retry attempts on upstream connection failure |

### spec.realIp

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trustedIps` | `Array<String>` | `[]` | List of trusted proxy IPs or CIDRs |
| `realIpHeader` | `String` | `X-Forwarded-For` | Header from which to extract the real client IP |
| `recursive` | `Boolean` | `true` | Traverse the header right-to-left, skip trusted proxies, and take the first non-trusted IP |

Extraction logic: walk `realIpHeader` from right to left, skip addresses in `trustedIps`, and return the first non-trusted IP.

### spec.securityProtect

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `xForwardedForLimit` | `Integer` | `200` | Maximum byte length of the `X-Forwarded-For` header; excess is truncated |
| `requireSniHostMatch` | `Boolean` | `true` | HTTPS listener-level 421 Misdirected Request detection; rejects requests where SNI does not match the Host header |
| `fallbackSni` | `String` | — | Fallback hostname used when the client provides no SNI |
| `tlsProxyLogRecord` | `Boolean` | `true` | Record TLS proxy connection logs |
| `allowLoopbackUpstream` | `Boolean` | `false` | Explicitly allow loopback upstream addresses for this GatewayClass. Link-local / metadata ranges remain blocked |

### spec.globalPluginsRef

Global plugins applied to all routes under this GatewayClass, executed before route-level plugins.

```yaml
globalPluginsRef:
  - name: "global-cors"       # EdgionPlugins resource name
    namespace: "edgion-system" # Defaults to "default"
```

### spec.preflightPolicy

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | `String` | `cors-standard` | `cors-standard`: intercept OPTIONS requests with `Origin` + `ACRM`; `all-options`: intercept all OPTIONS requests |
| `statusCode` | `Integer` | `204` | Preflight response status code when no CORS plugin is attached |

### spec.enableReferenceGrantValidation

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enableReferenceGrantValidation` | `Boolean` | `false` | Enforce ReferenceGrant validation for cross-namespace references |

## Common Scenarios

### Configuring Real IP Extraction

When Gateway is deployed behind a load balancer, configure trusted proxy IPs to correctly extract the client's real IP:

```yaml
spec:
  realIp:
    trustedIps:
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
