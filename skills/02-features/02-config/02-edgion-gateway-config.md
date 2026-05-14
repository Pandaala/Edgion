---
name: edgion-gateway-config-crd
description: EdgionGatewayConfig CRD schema: GatewayClass-level runtime configuration.
---

# EdgionGatewayConfig CRD Schema

> API Group: `edgion.io/v1alpha1` | Scope: Cluster | Referenced via GatewayClass.spec.parametersRef

## When to Change This Layer

Change here, instead of the Gateway TOML, when:

- A specific GatewayClass needs its own server / timeout / security defaults
- You want to configure global plugin references
- You want to enable a preflight policy
- You want real IP / ReferenceGrant validation as a GatewayClass-level policy

Entry relationship: `GatewayClass.spec.parametersRef` -> `EdgionGatewayConfig`

## Files to Reference

- Example file: `examples/test/conf/base/EdgionGatewayConfig.yaml`
- Code definition: `src/types/resources/edgion_gateway_config.rs`

## How to Associate

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
    name: default-config
```

## Full Schema

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  # ─── Pingora Server configuration ───
  server:
    threads: 0                              # uint32, default = CPU core count
    workStealing: true                      # bool
    gracePeriodSeconds: 30                  # uint64
    gracefulShutdownTimeoutS: 10            # uint64
    upstreamKeepalivePoolSize: 128          # uint32
    errorLog: ""                            # string, optional
    enableCompression: false                # bool, downstream response compression
    downstreamKeepaliveRequestLimit: 1000   # uint32, 0 = unlimited

  # ─── HTTP timeouts ───
  httpTimeout:
    client:
      readTimeout: "60s"                    # Duration string
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"

  # ─── Max retries ───
  maxRetries: 3                             # uint32, max upstream connection retries

  # ─── Real IP extraction ───
  realIp:
    trustedIps:                             # Vec<IpGroup>, list of trusted proxy IP groups
      - name: private-ranges                # string, required; <=63 chars, charset [a-zA-Z0-9._-], starts/ends alphanumeric, unique within the list
        description: ""                     # string, optional
        cidrs:                              # Vec<String>, required; non-empty list of IPs/CIDRs
          - "10.0.0.0/8"
    realIpHeader: "X-Forwarded-For"         # string, header used to extract the real IP
    recursive: true                         # bool, traverse the header right-to-left, skip trustedIps, take the first untrusted IP

  # ─── Security protection ───
  securityProtect:
    xForwardedForLimit: 200                 # usize, max XFF byte length
    requireSniHostMatch: true               # bool, HTTPS listener-level 421 Misdirected Request detection
    fallbackSni: ""                         # string?, fallback value when the client provides no SNI
    tlsProxyLogRecord: true                 # bool, record TLS proxy connection logs

  # ─── Global plugins ───
  globalPluginsRef:                         # Vec<PluginReference>, applied to all routes
    - name: "global-cors"
      namespace: "edgion-system"            # default "default"

  # ─── Preflight policy ───
  preflightPolicy:
    mode: "cors-standard"                   # "cors-standard" | "all-options"
    statusCode: 204                         # uint16, response status when no CORS plugin is present

  # ─── ReferenceGrant validation ───
  enableReferenceGrantValidation: false     # bool

  # ─── Path normalization ───
  pathNormalization:
    rejectEscapedSlashes: false             # bool, whether requests containing %2F / %5C are rejected with 400. Default false.
    rejectInvalidPercentEncoding: false     # bool, whether requests with invalid percent-encoding (invalid hex or non-UTF-8 after decoding) are rejected with 400. Default false.

  # ─── LinkSys global configuration ───
  linkSys:
    webhookMaxResponseBytes: 16384          # usize, hard upper bound for webhook response body (default 16 KiB)
```

## spec Field Details

### spec.server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `threads` | `u32` | CPU core count | Pingora worker threads |
| `workStealing` | `bool` | `true` | Task stealing |
| `gracePeriodSeconds` | `u64` | `30` | Graceful shutdown wait |
| `gracefulShutdownTimeoutS` | `u64` | `10` | Shutdown timeout |
| `upstreamKeepalivePoolSize` | `u32` | `128` | Upstream connection pool |
| `errorLog` | `String?` | — | Pingora error log |
| `enableCompression` | `bool` | `false` | Downstream response compression |
| `downstreamKeepaliveRequestLimit` | `u32` | `1000` | Maximum requests per downstream connection |

### spec.httpTimeout.client

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `readTimeout` | `Duration` | `60s` | Timeout for reading the client request |
| `writeTimeout` | `Duration` | `60s` | Timeout for writing the client response |
| `keepaliveTimeout` | `Duration` | `75s` | HTTP keepalive timeout |

### spec.httpTimeout.backend

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `defaultConnectTimeout` | `Duration` | `5s` | Upstream connect timeout |
| `defaultRequestTimeout` | `Duration` | `60s` | Total request timeout (including retries) |
| `defaultIdleTimeout` | `Duration` | `300s` | Connection pool idle timeout |

### spec.realIp

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `trustedIps` | `Vec<IpGroup>` | `[]` | List of trusted proxy IP groups |
| `realIpHeader` | `String` | `X-Forwarded-For` | Header name from which the real IP is extracted |
| `recursive` | `bool` | `true` | Whether to skip trusted proxies right-to-left in nginx style |

Each `trustedIps` entry is an `IpGroup`: `name` (required; ≤63 chars, charset `[a-zA-Z0-9._-]`, starts/ends alphanumeric, unique within the list), `description` (optional), `cidrs` (required; non-empty list of IP/CIDR strings).

Extraction logic: traverse `realIpHeader` right-to-left, skip addresses listed in any `trustedIps` group, and take the first untrusted IP.

### spec.securityProtect

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `xForwardedForLimit` | `usize` | `200` | Max byte length of X-Forwarded-For |
| `requireSniHostMatch` | `bool` | `true` | HTTPS listener-level 421 Misdirected Request detection (can also be enabled per-Gateway via Gateway annotations) |
| `fallbackSni` | `String?` | — | Fallback hostname when the client does not provide SNI |
| `tlsProxyLogRecord` | `bool` | `true` | Record TLS proxy connection logs |
| `allowLoopbackUpstream` | `bool` | `false` | Explicitly allow loopback upstream; link-local / metadata are not allowed |

### spec.preflightPolicy

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `String` | `cors-standard` | `cors-standard`: OPTIONS + Origin + ACRM; `all-options`: all OPTIONS requests |
| `statusCode` | `u16` | `204` | Preflight response status code when no CORS plugin is present |

### spec.pathNormalization

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `rejectEscapedSlashes` | `bool` | `false` | Reject requests containing `%2F` / `%5C` directly with 400. Can be enabled in high-security scenarios (where literal `/` characters in the path are not allowed). Note: this affects legitimate APIs (e.g., base64 URL parameters containing `/`). |
| `rejectInvalidPercentEncoding` | `bool` | `false` | Reject requests with invalid percent-encoding (invalid hex or non-UTF-8 after decoding) directly with 400. Once enabled, captures abnormal inputs such as IIS-style overlong UTF-8 (`%c0%ae`), broadening the security guarantee scope. |

**Important notes**:

- **No `enabled` field** — path normalization is Edgion's default behavior and **cannot be disabled**.
- **No per-Gateway / per-Listener override** — these two switches are global.
- For the security guarantee scope see spec §1.4: [tasks/todo/uri-normalization.md](../../../tasks/todo/uri-normalization.md).

### spec.linkSys

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `webhookMaxResponseBytes` | `usize` | `16384` | Hard upper bound for the LinkSys webhook response body (bytes). All webhook call response sizes are clamped to `min(per-resource maxResponseBytes, this value)`. When larger responses are needed (e.g., a plugin calling an external API that returns a sizable JSON), increase both values together. |

> Modifications take effect immediately via `EdgionGatewayConfig` partial-update; no Gateway restart is required.

### spec.globalPluginsRef

```yaml
globalPluginsRef:
  - name: String        # EdgionPlugins resource name
    namespace: String?   # default "default"
```

Global plugins are applied to all routes and execute before route-level plugins.

## Differences from TOML Configuration

| Configuration Item | TOML | EdgionGatewayConfig |
|--------------------|------|---------------------|
| Scope | A single process instance | The entire GatewayClass |
| Change Method | Restart the process | Takes effect dynamically (CRD update) |
| Suitable For | Process connection parameters, log paths | Business-level timeouts, security policies, global plugins |

## When Not to Change This Layer

Do not use `EdgionGatewayConfig` to solve these problems:

- Which Controller the Gateway process connects to: that is `server_addr` in the Gateway TOML.
- Gateway Admin / Metrics ports: that is the process startup layer.
- Whether the Controller uses file_system or kubernetes: that is `conf_center` in the Controller TOML.
