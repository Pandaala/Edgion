---
name: edgion-gateway-config-crd
description: EdgionGatewayConfig CRD schema: GatewayClass-level runtime configuration.
---

# EdgionGatewayConfig CRD Schema

> API Group: `edgion.io/v1alpha1` | Scope: Cluster | Referenced via GatewayClass.spec.parametersRef

## When to Change This Layer

Change here, instead of the Gateway YAML, when:

- A specific GatewayClass needs its own server / timeout / security defaults
- You want to configure global plugin references or dynamic plugin execution policy
- You want to enable a preflight policy
- You want real IP / ReferenceGrant validation as a GatewayClass-level policy

Entry relationship: `GatewayClass.spec.parametersRef` -> `EdgionGatewayConfig`

## Files to Reference

- Example file: `edgion-tests/integration/conf/base/EdgionGatewayConfig.yaml`
- Code definition: `edgion-resources/src/resources/edgion_gateway_config.rs`

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
  # ─── GatewayClass HTTP server behavior (dynamic) ───
  server:
    enableCompression: false                # bool, gzip known-length downstream responses
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

  # ─── L4 TCP relay timeouts (UDP is unaffected) ───
  tcpTimeout:
    idleTimeout: "1h"
    connectTimeout: "10s"

  # ─── Retry defaults ───
  retry:
    attempts: 2                            # retries after the initial request; range 0..=i32::MAX

  # ─── Load balancing ───
  loadBalancing:
    degradeThreshold: 0                       # uint8 0-100; default 0 (disabled). When the healthy-backend fraction
                                            # drops below this percent, ignore health filtering and route across all
                                            # backends. 0 = disabled. Per-Service annotation overrides this value.

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
    allowLoopbackUpstream: false            # bool, explicit loopback upstream opt-in
    rejectDuplicateHost: true               # bool, reject duplicate HTTP/1.1 Host fields

  # ─── Request-body buffering policy ───
  requestBody:
    enabled: true
    defaultMemoryBufferSize: "128KiB"
    # maxMemoryBufferSize: "1MiB"            # optional effective override
    defaultMaxBodySize: "32MiB"
    # maxBodySize: "64MiB"                   # optional effective override
    storageOperationTimeout: "5s"            # admission + one file operation

  # ─── Global plugins ───
  globalPluginsRef:                         # Vec<PluginReference>, applied to all routes
    - name: "global-cors"
      namespace: "edgion-system"            # default "default"

  # ─── Access log extension masking whitelist ───
  accessLogExtern:
    unmaskedKeys:
      header:     [user-agent, origin, content-type, content-length, accept, x-request-id]  # default set
      respHeader: [content-type, content-length, cache-control, retry-after, x-request-id]   # default set
      query: []                             # default empty; key names are 100% business-defined
      cookie: []                            # default empty
      ctx: []                               # default empty

  # ─── Preflight policy ───
  preflightPolicy:
    mode: "cors-standard"                   # "cors-standard" | "all-options"
    statusCode: 204                         # final HTTP status (200-599) when no CORS plugin is present

  # ─── LinkSys global configuration ───
  linkSys:
    webhookMaxResponseBytes: 16384          # usize, hard upper bound for webhook response body (default 16 KiB)
    maxInstancesPerKind: 200                # usize, max current registered instances per persistent provider kind

  # ─── Cluster-wide outbound TLS defaults for plugins and LinkSys ───
  outboundTls:
    verify: true
    validation:
      wellKnownCACertificates: System
      # hostname: api.example.com
      # caCertificateRefs: [{ kind: ConfigMap, name: corporate-ca }]
    # clientCertificateRef: { kind: Secret, name: outbound-client }

  # ─── Data-plane ExternalName DNS fallback ───
  dnsResolver:
    servers: ["1.1.1.1:53"]                # Mutually exclusive with linkSysRef
    # linkSysRef: { namespace: edgion-system, name: corporate-httpdns }
    cacheTtl: "5s"
```

## spec Field Details

### spec.server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enableCompression` | `bool` | `false` | Gzip eligible downstream responses with a valid `Content-Length` and no `Transfer-Encoding`; unknown-length and SSE responses remain uncompressed |
| `downstreamKeepaliveRequestLimit` | `u32` | `1000` | Maximum requests per downstream connection |

Compression is captured per HTTP request. The keepalive budget is captured by the
first HTTP/1 request and retained by that connection; existing connections do not
adopt a later budget. Both fields update without listener reconstruction.
Process settings (`threads`, `work_stealing`, shutdown periods,
`upstream_keepalive_pool_size`, and `error_log`) belong exclusively to the local Gateway YAML.

### Runtime publication boundaries

Edgion propagates referenced EGC changes to existing Gateways. The single EGC
`ArcSwap` snapshot publishes `Arc<ResolvedGatewayConfig>` entries keyed by name.
Listeners bind once to a stable per-name `GatewayConfigHandle`; publication
updates its ArcSwap under the same lock used for binding. HTTP request creation
loads directly from that handle without a name lookup and keeps the Arc through retries and response
processing. This covers timeouts, retries, RealIp, security checks, body policy,
forwarded headers, preflight, and compression. Parsing and matcher construction
occur at publication, never on the request path.

TCP and both TLS proxy modes capture at `process_new`; UDP captures only when a
session is created; fallback SNI is selected from a handshake snapshot. Existing
consumers keep their generation. Missing/deleted EGC references use runtime
defaults, while scoped plugin admission independently fails closed for a missing
referenced EGC. No reference is distinct from an EGC literally named `default`.

Plugin policy/global plugins/access-log masking and shared LinkSys, load-balancing,
outbound TLS, and DNS mechanisms retain their existing dynamic publication scopes.
They are not part of an atomic transaction with the HTTP request Arc. This does
not retarget GatewayClass references or change physical listener ownership.

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

### spec.tcpTimeout

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `idleTimeout` | `Duration` | `1h` | Established L4 relay inactivity timeout; `0s` disables it. Applies to TCP, TLS termination, and TLS passthrough, not UDP. |
| `connectTimeout` | `Duration` | `10s` | Upstream TCP connection bound; `0s` delegates to the OS bound. |

Updates are parsed when the Gateway receives the EdgionGatewayConfig generation.
Each TCP, TLS-terminated, or TLS-passthrough connection copies the latest resolved
pair when it enters L4 proxy processing, so later connections adopt an update
without rebinding the listener. An in-flight connection keeps its original pair
through all connect retries and the relay. Changing a GatewayClass `parametersRef`
or the owner of a shared physical listener still requires a process restart.

### spec.loadBalancing

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `degradeThreshold` | `u8` | `0` | Global degrade threshold percent (0–100). When the fraction of healthy backends for a service (after health-check + outlier-detection filtering) drops below this value, the gateway routes across all backends instead of returning 503. `0` disables degrade mode. A per-Service `EdgionBackendTrafficPolicy`'s `spec.loadBalancer.degradeThreshold` overrides this global default for its target Service(s). Prometheus counter `edgion_lb_degrade_selections_total{service}` counts selections entering the degrade branch across Service port sections. Retries count separately; this is not a current-state gauge. |

> Default `0` matches Istio's Kubernetes default. Enable (e.g. `50`) only for services with larger backend pools (≈ ≥ 5 endpoints); see [03-resources/20-edgion-backend-traffic-policy.md](../03-resources/20-edgion-backend-traffic-policy.md) for the full rationale and the per-Service override.

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
| `xForwardedForLimit` | `usize` | `200` | Max byte length of X-Forwarded-For; excess returns HTTP 400 |
| `requireSniHostMatch` | `bool` | `true` | HTTPS listener-level 421 Misdirected Request detection (can also be enabled per-Gateway via Gateway annotations) |
| `fallbackSni` | `String?` | — | Fallback hostname when the client does not provide SNI |
| `tlsProxyLogRecord` | `bool` | `true` | Record TLS proxy connection logs |
| `allowLoopbackUpstream` | `bool` | `false` | Explicitly allow loopback upstream; link-local / metadata are not allowed |
| `rejectDuplicateHost` | `bool` | `true` | Reject HTTP/1.1 requests containing multiple Host field lines with 400. |

Omitting the entire `securityProtect` block disables the XFF byte cap. A present
block with an omitted `xForwardedForLimit` uses 200. New HTTP requests use updated
XFF, SNI/Host, duplicate-Host and loopback policies; new TCP/TLS connections and
UDP sessions use updated loopback policies. TLS proxy logging is connection-pinned,
and fallback SNI applies to new handshakes.

### spec.requestBody

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `true` | Runtime capture switch. When false, body-aware plugins receive the normal unimplemented read error; bodyless reads remain empty. |
| `defaultMemoryBufferSize` | `String` | `128KiB` | Default bytes retained in memory before disk spill. |
| `maxMemoryBufferSize` | `String?` | unset | Effective business override for the memory spill threshold. Falls back to `defaultMemoryBufferSize`. |
| `defaultMaxBodySize` | `String` | `32MiB` | Default physical complete-body cap. |
| `maxBodySize` | `String?` | unset | Effective business override for the physical body cap. Falls back to `defaultMaxBodySize`; per-plugin `body.maxBodySize` may tighten it further for that plugin only. |
| `storageOperationTimeout` | `String` | `5s` | Non-zero GEP-2257 duration bounding storage-I/O admission plus one spill, replay, materialization, or snapshot file operation. |

All sizes must be positive human-readable byte sizes. The effective memory
threshold must not exceed the effective body cap. The Gateway resolves these
values when publishing an `EdgionGatewayConfig` update. Each new request copies
the resolved boolean, integer, and duration fields from an immutable snapshot;
it never parses sizes/durations or takes a blocking lock. Storage deadline
expiry is a gateway-local storage failure, not a downstream read timeout.

### spec.preflightPolicy

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `String` | `cors-standard` | Global selection: `cors-standard` selects OPTIONS + Origin + ACRM; `all-options` selects all OPTIONS. A route CORS filter still decides local response ownership. |
| `statusCode` | `u16` | `204` | Final preflight response status (200-599) when no CORS plugin is present |

### spec.linkSys

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `webhookMaxResponseBytes` | `usize` | `16384` | Hard upper bound for the LinkSys webhook response body (bytes). All webhook call response sizes are clamped to `min(per-resource maxResponseBytes, this value)`. When larger responses are needed (e.g., a plugin calling an external API that returns a sizable JSON), increase both values together. |
| `maxInstancesPerKind` | `usize` | `200` | Max current registered persistent LinkSys instances per provider kind (range 1–10000). Unadmitted keys are retried after removal or a cap increase. Updates and deletes proceed without waiting for old cleanup; retiring objects are not counted. Lowering the cap does not evict current instances. |

> Modifications take effect immediately via `EdgionGatewayConfig` partial-update; no Gateway restart is required.

### spec.outboundTls

Cluster-wide default for plugin and LinkSys outbound TLS. `verify` defaults to `true`.
`validation` selects well-known roots or referenced Secret/ConfigMap CA bundles and may
add hostname/SAN constraints. `clientCertificateRef` supplies an outbound mTLS identity.
Controller-resolved certificate material stays out of the CRD schema and is redacted in
Admin output while remaining available to conf_sync.

### spec.dnsResolver

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `linkSysRef` | `LinkSysRef?` | — | Namespaced httpdns LinkSys reference. |
| `servers` | `Vec<String>?` | — | Inline `ip[:port]` DNS servers. |
| `cacheTtl` | `Duration` | `5s` | Data-plane ExternalName resolution cache TTL. |

`linkSysRef` and `servers` are mutually exclusive and validated at ingest. This is the
data-plane fallback; Controller-side resolution uses Controller YAML.

### spec.pluginPolicy

The sole runtime execution policy for plugins attached to this GatewayClass.
`GatewayClass.spec.parametersRef` selects this cluster-scoped resource. Updating
it publishes a new policy and compatible executable views without restarting
Controller, Gateway, or listeners. A request captures its logical Gateway scope;
new stream connections and UDP sessions resolve policy at admission.

```yaml
pluginPolicy:
  # allow: []                 # Presence means allow-list mode; empty denies all
  deny:
    - name: http/Wasm
    - name: http/Dsl
      action: block
      blockStatus: 451
    - name: stream/ConnectionRateLimit
      action: bypass
  defaultAction: allow        # allow | deny for unmatched names without allow
  deniedAction: block         # block | bypass for restricted names
  blockStatus: 503            # 400..599, HTTP only
```

| Field | Default | Meaning |
|---|---|---|
| `allow` | absent | Exact qualified names allowed to execute; an explicit empty list denies every name |
| `deny` | `[]` | Exact-name restrictions, evaluated before `allow` |
| `defaultAction` | `allow` | Decision for names unmatched by a configured list |
| `deniedAction` | `block` | Execution behavior for restrictions without an entry override |
| `blockStatus` | `503` | Default HTTP rejection status |
| `deny[].action` | inherited | Overrides `deniedAction` |
| `deny[].blockStatus` | inherited | Overrides status; forbidden when the effective entry action is `bypass` |

Qualified names are case-sensitive `<family>/<Type>` from the actual enums:
`http` covers all four EdgionPlugins stages, `stream` covers connection plugins,
and `tls-route` covers TLSRoute plugins. There are no wildcard names or aliases.
Unknown names, duplicates and invalid status values are rejected. `allow` has no
CRD default: omission and an explicit empty list have different meanings.

No parametersRef, or a present EGC with no pluginPolicy, means allow all. A
referenced EGC that is missing, deleted, or has a semantically invalid policy is
unavailable: HTTP admission returns 503 and stream admission denies. API-server
or deserialization rejection does not publish a new typed resource. EGC status
reports Controller validation, not Gateway acknowledgement or fleet convergence.

A restricted entry is absent from the scoped executable view and is never
constructed solely for a denied scope. Stateful entries are shared by allowed
scopes of the same source generation. Block precedes conditions, invalid-program
gates and matched-rule AI checks. Bypass skips execution; bypassing authentication
also removes that authentication check. Native Gateway API filters and the
resource-level native WAF are outside the named plugin catalog.

Invalid DSL/WAF updates may retain permitted entries from the previous valid
source. Current policy applies to both sources; a new restriction cannot be lost
to retention. Relaxing policy while the latest source remains invalid cannot
resurrect an old excluded entry and may return the ordinary invalid-config 500.
Existing admitted requests and sessions are not revoked. There is no cross-kind
or fleet-wide atomic update guarantee.

See [the operator guide](../../../docs/en/ops-guide/plugin-policy.md).

### spec.globalPluginsRef

```yaml
globalPluginsRef:
  - name: String        # EdgionPlugins resource name
    namespace: String?   # default "default"
```

Global plugins are applied to all routes and execute before route-level plugins.

### spec.accessLogExtern

The masking whitelist for `AccessLogExtern`-declared access-log fields
(`EdgionPlugins.spec.accessLogExtern`). Platform-owned: it lives here —
cluster-scoped, next to `globalPluginsRef` — rather than in a namespaced
resource, so a tenant who owns a route and its own `EdgionPlugins` cannot
widen masking for the fleet. See
[`../../03-coding/observability/00-access-log.md`](../../03-coding/observability/00-access-log.md)
for the full `AccessLogExtern` design.

```yaml
accessLogExtern:
  unmaskedKeys:
    header:     [user-agent, origin, content-type, content-length, accept, x-request-id]
    respHeader: [content-type, content-length, cache-control, retry-after, x-request-id]
    query:      []
    cookie:     []
    ctx:        []
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `unmaskedKeys.header` | `Vec<String>` | `[user-agent, origin, content-type, content-length, accept, x-request-id]` | Request header names logged in clear. Matched case-insensitively. |
| `unmaskedKeys.respHeader` | `Vec<String>` | `[content-type, content-length, cache-control, retry-after, x-request-id]` | Response header names logged in clear. Matched case-insensitively. |
| `unmaskedKeys.query` | `Vec<String>` | `[]` | Query parameter names logged in clear. Case-sensitive. |
| `unmaskedKeys.cookie` | `Vec<String>` | `[]` | Cookie names logged in clear. Case-sensitive. |
| `unmaskedKeys.ctx` | `Vec<String>` | `[]` | Ctx-var names logged in clear. Case-sensitive. |

**Keyed by source type, not a flat list**: `user-agent` as a header is
harmless, but a cookie of the same name is a different object — a flat list
could not distinguish them.

**Why `query`/`cookie`/`ctx` default empty and `header`/`respHeader` don't.**
The header/respHeader defaults are fields every access log already effectively
carries and that identify no one: content negotiation plus `x-request-id` for
correlation. `referer` and `location` are deliberately excluded even though
they are common in other gateways' combined-log defaults — `referer` is
someone else's full URL, often carrying a session id in its query, and
`location` can carry OAuth implicit-flow tokens or presigned-URL signatures.
`query`/`cookie`/`ctx` default to empty because their key names are entirely
business-defined; there is no name that is universally safe across clusters
(`page` looks harmless until the next tenant uses it for something else).

**Omitting a list keeps that list's default, it does not empty it.**
`UnmaskedKeys` is `#[serde(default)]` at the container level, so each
*missing* list is independently filled from `UnmaskedKeys::default()` —
which is non-empty for `header`/`respHeader`. So
`accessLogExtern: {unmaskedKeys: {ctx: [tenant_id]}}` — the natural way to
write "only `tenant_id` is unmasked" — also keeps the default 6 request
headers and 5 response headers unmasked. To unmask only `tenant_id`, list
every category explicitly and write `header: []` (and `respHeader: []`) to
drop the header defaults.

**A whitelist hit bypasses masking; a miss does not drop the field** — it
still emits the key, rendered per `access_log_sensitive.mode` (see
[`01-gateway-config.md`](01-gateway-config.md#access_log_sensitive)). Category
1 sources (`routeLabel`/`routeAnnotation`) bypass this whitelist entirely and
are always clear — they are operator-authored route configuration, not
request data.

**Hard blocklist is separate from this whitelist and is not configurable
here.** Names like `authorization`, `set-cookie`, `x-api-key` (`header`), and
`jwt_claims`/`oidc_claims`/`jwe_payload` (`ctx`) cannot be named regardless of
this whitelist — widening them requires a code change, not a CR edit.
Enforcement is two-part: a declaration naming a blocked key is **reported**
on the resource's `Accepted=False` status condition at `EdgionPlugins`
ingest (admission is lenient — the resource still lands in the cache
conf_sync serves to the gateway), and then actually **dropped at gateway
load** by `compile_plugins`'s `retain()`
(`edgion-gateway/src/plugins/http/plugin_store.rs`), which discards any
declared field whose `validation_error()` is `Some` before the spec is
stored. The drop, not the report, is what keeps the key out of the log.

**Scope caveat.** `bound_listeners` dedup (`runtime/server/base.rs`) skips a
listener already bound at the same addr/protocol, so when two Gateways of
different GatewayClasses share an addr:port, the first Gateway's
`EdgionGatewayConfig` — including this whitelist — wins for that listener.
The whitelist is therefore effectively **per-listener**, not per-Gateway.
Document this in deployment topology; there is no mechanism enforcing
per-Gateway isolation here.

## Differences from Gateway YAML Configuration

| Configuration Item | Gateway YAML | EdgionGatewayConfig |
|--------------------|------|---------------------|
| Scope | A single process instance | The entire GatewayClass |
| Change Method | Restart the process | Takes effect dynamically (CRD update) |
| Suitable For | Process connection parameters, log paths | Business-level timeouts, security policies, global plugins |

## When Not to Change This Layer

Do not use `EdgionGatewayConfig` to solve these problems:

- Which Controller the Gateway process connects to: that is `server_addrs` in the Gateway YAML.
- Gateway Admin / Metrics ports: that is the process startup layer.
- Path-normalization rejection switches: those are `path_normalization` in Gateway YAML
  and require restart. They are not CRD fields; adding them here would be pruned by the
  Kubernetes structural schema.
- Whether the Controller uses file_system, etcd, or kubernetes: that is `conf_center` in the Controller YAML.
