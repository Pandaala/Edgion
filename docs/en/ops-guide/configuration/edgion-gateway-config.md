# EdgionGatewayConfig

EdgionGatewayConfig is a cluster-scoped CRD (`edgion.io/v1alpha1`) that configures behavior at the GatewayClass level. It is referenced via `GatewayClass.spec.parametersRef`. Updates propagate to existing Gateways without restarting the process; the field determines which new request, connection, or operation adopts the update.

## Optional expensive WAF checks

`spec.waf.expensiveChecks` defaults to `false`. Set it to `true` to enable
compiler-classified checks that retain broad cross-field aggregate state:

```yaml
spec:
  waf:
    expensiveChecks: true
```

The current CRS 4.25.1 classes are the name histogram/HPP pair (921170/921180)
and the Multipart Content-Type list (922140/922150/922110). OFF omits eligible
aggregate initialization, retained list/counter writes and checking. The Multipart
collector still performs its ordinary matching and capture updates, preserving
numeric slots and earlier named-capture cleanup for other rules.

The policy applies to new requests across query, form, multipart and JSON inputs.
Existing requests keep their captured policy. Cheap count/length checks, normal
attack matching, mandatory JSON key identity and XML grammar/duplicate attributes
remain enabled. The current supported XML corpus has no independently removable
aggregate-record class. Strict source validation always runs. Changed or partial
families and unknown/custom namespace dependencies conservatively retain their
required computation; IDs alone never determine eligibility.

Exact canonical restricted-header and RFI checks remain active. Removing aggregate
state/checks also removes incidental inputs through the original unanchored TX
selectors, including their possible errors. This is an explicit optional-check
coverage tradeoff, not universal equivalence to all-original rules. Standard PL1
already skips HPP; actual savings depend on profile and input. The switch does not
remove mandatory parser arenas, part-header publication or all temporary values.
Deleting a referenced EGC still follows existing plugin-scope admission denial.

## Difference from Gateway YAML

| Layer | Gateway YAML | EdgionGatewayConfig |
|-------|-------------|---------------------|
| Scope | Single Gateway process | Entire GatewayClass |
| How changes apply | Requires process restart | Dynamic, at the field's request, connection, or operation boundary |
| Typical use | Process connection params, log paths, rate limit precision | Timeout policies, security rules, global plugins, real IP extraction |

**When to use Gateway YAML vs EdgionGatewayConfig:**
- Change the Controller gRPC address → Gateway YAML `gateway.server_addrs`
- Change Admin / Metrics ports → Gateway YAML (process startup layer)
- Configure HTTP timeouts, real IP, global plugins → EdgionGatewayConfig

## How to Associate

Reference EdgionGatewayConfig from a GatewayClass via `parametersRef`:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Cluster
  data:
    type: IpList
    config:
      items:
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
---
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
    downstreamKeepaliveRequestLimit: 10000

  httpTimeout:
    client:
      readTimeout: "60s"
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"

  retry:
    attempts: 2

  realIp:
    trustedIpRefs:
      - name: realip-proxies-1
        namespace: default
    realIpHeader: "X-Forwarded-For"
    recursive: true

  securityProtect:
    xForwardedForLimit: 200
    requireSniHostMatch: true
    fallbackSni: ""
    tlsProxyLogRecord: true

  requestBody:
    enabled: true
    maxBodySize: "32MiB"
    storage:
      memoryLimit: "128KiB"
      spillToDisk: true
      # maxBufferedSize: "32MiB"
      operationTimeout: "5s"

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
    maxInstancesPerKind: 200         # Max current registered instances per persistent provider kind
```

> Note: URI path normalization (`%2F` rejection, invalid percent-encoding rejection, etc.) is **not** part of EdgionGatewayConfig — it is configured in the Gateway YAML `path_normalization` section. See [Gateway Configuration](./gateway-config.md).

## Hot Update Boundaries

Edgion propagates changes to the referenced EGC into existing Gateways. This is an
Edgion behavior: Gateway API does not require GatewayClass parameter changes to
propagate to Gateways that already exist.

Listeners retain a shared configuration handle. An EGC update replaces its
resolved snapshot; each new request captures that snapshot directly without
looking up the EGC name. Deletion and same-name recreation update the same handle.

| Configuration | When the updated value applies |
|---|---|
| HTTP timeouts, retries, RealIp, XFF and SNI/Host checks, request body, forwarded headers, preflight, compression, HTTP loopback permission | Each new HTTP request captures one configuration generation and keeps it through retries and response processing |
| `server.downstreamKeepaliveRequestLimit` | The first HTTP/1 request captures a budget for its connection; later requests on that connection retain it |
| `tcpTimeout`, TCP/TLS loopback permission, `tlsProxyLogRecord` | Each new L4 proxy connection captures its policy and retains it through retries, relay, and logging |
| UDP loopback permission | Each newly created UDP session; existing sessions retain their policy |
| `fallbackSni` | Each new TLS handshake; an established TLS connection does not select a new certificate |
| Plugin policy, global plugins, access-log masking, LinkSys, load-balancing defaults, outbound TLS, DNS | Their existing dynamic publishers and consumers apply updates with their own scopes and lifetimes |

These mechanisms do not form one atomic update across resource kinds or Gateway
replicas. An in-flight request does not change its captured EGC generation, but
independently published shared services retain their own update semantics.

Removing a referenced EGC restores the built-in request/connection defaults for
new consumers. Plugin admission independently fails closed for a missing referenced
EGC. Changing the GatewayClass reference itself, adding listener ports, transferring
shared-listener ownership, or deleting and recreating a Gateway remains outside
this hot-update contract.

## Configuration Parameters

### spec.server

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enableCompression` | `Boolean` | `false` | Gzip eligible downstream responses with a valid `Content-Length` and no `Transfer-Encoding`; unknown-length and SSE responses remain uncompressed to preserve incremental delivery |
| `downstreamKeepaliveRequestLimit` | `Integer` | `10000` | Maximum requests per downstream HTTP/1 connection; `0` means unlimited |

Compression is captured per HTTP request. The keepalive request budget is captured
by the first HTTP/1 request and retained for that connection. Both support updates
without restarting listeners. Pingora worker threads, shutdown periods, the upstream
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

### spec.retry

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `attempts` | `Integer` | `2` | Maximum retries after the initial upstream attempt. `0` disables retries; the supported range is `0..=2147483647` |

The initial upstream request does not consume this retry budget, so the maximum
number of upstream requests is `1 + attempts`. A route-level
`rules[].retry.attempts` value overrides this GatewayClass-wide default.

### spec.realIp

| Parameter | Default | Description |
| --- | --- | --- |
| `trustedIpRefs` | required | Nonempty ordered IpList references; every entry requires `name` and explicit `namespace` |
| `realIpHeader` | `X-Forwarded-For` | Valid HTTP field name, at most 256 bytes |
| `recursive` | `true` | Walk valid forwarded hops right-to-left; all-trusted chains return the leftmost parsed hop |
| `maxTrustedHops` | unlimited | Recursive cap; 0 stops at the first trusted hop; ignored in nonrecursive mode |

Store IP/CIDR collections in the referenced IpList's `items` (IpGroups). Global EGC references require current `Cluster` visibility when ReferenceGrant validation is enabled; namespaced grants and the outbound TLS administrative exemption do not authorize this trust policy. Validation-disabled resolution still checks identity, type and enabled state.

All references must be available before trusting the PROXY-effective peer or any forwarded hop. Missing, disabled, denied or wrong-type lists use the peer for that decision. One ConfigData snapshot covers all hops; content, visibility, deletion and restoration affect new decisions without copying CIDRs or rebuilding listeners. The central IpList compiler retains valid entries and excludes invalid ones; empty/all-invalid matchers trust no addresses.

Intrinsic errors in a typed, published configuration report `Accepted=False` and disable global extraction. Kubernetes required/minItems/namespace/header constraints may reject the update at API admission instead; that rejected update never reaches the Controller/Gateway and the previous admitted generation remains active. Removed inline `trustedIps` or unknown mixed keys survive narrow preservation and invalidate typed publication. Narrow CRD preservation allows typed invalid input to replace a previously valid runtime instead of silently pruning the old field. Clean publication restores extraction. Shared EGC status describes intrinsic structure only; deployment-local list availability adds no shared condition. Nonrecursive resolution uses the rightmost parsed hop if untrusted, otherwise the peer.

### spec.securityProtect

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `xForwardedForLimit` | `Integer` | `200` | Maximum byte length of the `X-Forwarded-For` header; exceeding it returns `400 Bad Request` |
| `requireSniHostMatch` | `Boolean` | `true` | HTTPS listener-level 421 Misdirected Request detection; rejects requests where SNI does not match the Host header |
| `fallbackSni` | `String` | — | Fallback hostname used when the client provides no SNI |
| `tlsProxyLogRecord` | `Boolean` | `true` | Record TLS proxy connection logs |
| `allowLoopbackUpstream` | `Boolean` | `false` | Explicitly allow loopback upstream addresses for this GatewayClass. Link-local / metadata ranges remain blocked |

Pingora rejects duplicate HTTP/1.1 `Host` fields during protocol parsing. This
behavior has no Gateway configuration switch.

If the entire `securityProtect` block is absent, the XFF byte cap is disabled.
If the block is present and `xForwardedForLimit` is omitted, its default is 200.

### spec.requestBody

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | `Boolean` | `true` | Runtime capture switch. When disabled, body-aware plugins fail closed with the normal body-unavailable result |
| `maxBodySize` | `String` | `32MiB` | Hard request-body ceiling |
| `storage.memoryLimit` | `String` | `128KiB` | Resident payload threshold before optional spill |
| `storage.spillToDisk` | `Boolean` | `true` | Allow disk spill above the memory threshold |
| `storage.maxBufferedSize` | `String` | inherit maxBodySize | New capture owner retention capacity |
| `storage.operationTimeout` | `String` | `5s` | Deadline for admission plus execution of one storage operation |

Require `memoryLimit <= maxBufferedSize <= maxBodySize`, with omitted buffer capacity inherited from the body ceiling. Disabling spill additionally bounds new complete captures by the memory threshold. Required complete consumers fail when capacity is exceeded; they never receive a truncated snapshot. A new owner's capacity also bounds subsequent logical mutations. Existing complete snapshots retain their original owner's capacity. Explicit WAF raw-prefix capture retains its independent at-most-1MiB memory window. Full WAF-only inspection can stop at a configured inspection or storage boundary without truncating forwarding; its `onLimit` selects continue or reject. Other complete-body consumers still require complete capture.

The physical ceiling also applies to ordinary HTTP without WAF or body plugins,
even when capture is disabled. Known Content-Length overflow rejects before
forwarding; unknown-length overflow stops the upload when native received bytes
exceed the cap. An already-selected response keeps ownership instead of being
replaced by 413. Canonical gRPC, CONNECT and successfully upgraded tunnels are
excluded; an Upgrade request header alone does not bypass admission.

All sizes must be positive human-readable byte sizes. The effective memory
threshold must not exceed the effective body cap. Values are parsed when the
configuration update is published. Each new request copies the resolved policy
from an immutable snapshot without parsing sizes or taking a blocking lock. A
plugin's `body.maxBodySize` may further tighten the limit for that plugin.
`storage.operationTimeout` uses the shared GEP-2257 duration syntax (`500ms`,
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

Masking is published with the scoped plugin runtime selected for the logical
Gateway. Its publication is independent of the listener's request/connection EGC
snapshot; shared-listener ownership does not replace that plugin scope.

### spec.preflightPolicy

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | `String` | `cors-standard` | `cors-standard`: select OPTIONS requests with `Origin` + `ACRM`; `all-options`: select all OPTIONS requests for global handling. A route CORS filter still decides whether to respond locally. See [Preflight Policy](../gateway/preflight-policy.md). |
| `statusCode` | `Integer` | `204` | Final preflight response status (200-599) when no CORS plugin is attached |

### spec.linkSys

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `webhookMaxResponseBytes` | `Integer` | `16384` | Hard upper bound (bytes) for a LinkSys webhook response body. Each call's response is clamped to `min(per-resource maxResponseBytes, this value)`. Raise both values together when an external API legitimately returns a larger payload |
| `maxInstancesPerKind` | `Integer` | `200` | Max current registered persistent LinkSys instances per provider kind (1–10000). Unadmitted keys are retried after removal or a cap increase. Updates and deletes do not wait for old cleanup; retiring objects are not counted. Lowering the cap does not evict current instances |

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
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Cluster
  data:
    type: IpList
    config:
      items:
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
        - 192.168.0.0/16
---
spec:
  realIp:
    trustedIpRefs:
      - name: realip-proxies-1
        namespace: default
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

## Backend DNS family default

```yaml
spec:
  dnsResolver:
    lookupFamily: ipv4Only
    cacheTtl: 5s
```

`lookupFamily` accepts `ipv4Only` (default), `ipv6Only`, or `all`. It may be set
without `servers` or `linkSysRef`. The existing selected global EGC owner publishes
this setting live together with its resolver candidate and TTL. Service then
Gateway annotations override it independently of resolver choice. AI external
hostnames use the Gateway override, then this default. Controller-owned slices use
the Controller/Service policy instead; no connection racing is introduced.
See the [backend family contract](../../user-guide/http-route/backends/service-ref.md#dns-address-families).

The DNS default owner is the existing deterministic minimum store-key winner across EGC resources defining DNS settings, not the current GatewayClass alone. A family-only DNS block participates in that selection.
