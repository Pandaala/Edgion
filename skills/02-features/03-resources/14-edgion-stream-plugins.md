---
name: stream-plugins-features
description: EdgionStreamPlugins CRD schema: connection-context plugins for TCP/UDP and a separate TLSRoute stage.
---

# EdgionStreamPlugins — Stream Plugins

> API: `edgion.io/v1` | Scope: Namespaced

EdgionStreamPlugins defines connection-stage plugins and a separate TLSRoute-stage chain.

## Full Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-my-stream-plugins-allow-1
  namespace: default
spec:
  data:
    type: IpList
    config:
      items:
      - name: internal-networks
        description: Private client networks
        cidrs:
        - 10.0.0.0/8
        - 192.168.1.0/24
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-my-stream-plugins-deny-2
  namespace: default
spec:
  data:
    type: IpList
    config:
      items:
      - name: blocked-hosts
        cidrs:
        - 10.0.0.100
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-my-stream-plugins-allow-3
  namespace: default
spec:
  data:
    type: IpList
    config:
      items:
      - name: tls-clients
        cidrs:
        - 172.16.0.0/12
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: my-stream-plugins
  namespace: default
spec:
  plugins:
  - enable: true
    type: IpRestriction
    config:
      allowRefs:
      - name: example-my-stream-plugins-allow-1
        namespace: default
      - name: corp-allowlist
      denyRefs:
      - name: example-my-stream-plugins-deny-2
        namespace: default
  tlsRoutePlugins:
  - enable: true
    type: IpRestriction
    config:
      allowRefs:
      - name: example-my-stream-plugins-allow-3
        namespace: default
```

## Two-Stage Model

| Stage | Trigger Time | Available Information | Field |
|-------|--------------|-----------------------|-------|
| Stage 1: connection context | Before TLS/HTTP processing for TCP-based listeners; when admitting a new UDPRoute session | Source IP, destination port | `spec.plugins` |
| Stage 2: TlsRoute | After route matching; post-handshake for Terminate, pre-forwarding for Passthrough | SNI and IP; TLS identity/mTLS only for Terminate | `spec.tlsRoutePlugins` |

TLSRoute executes only `tlsRoutePlugins`. An absent/null Stage-2 field with
nonempty raw `plugins` fails closed, even when those Stage-1 entries are disabled
or Bypass. Explicit `tlsRoutePlugins: []`, disabled/Bypass-only Stage-2, or both
fields absent/empty are no-ops; none invokes Stage-1. An enabled Stage-2 Block
entry denies. A missing referenced resource fails closed. See the
[stage contract](../../01-architecture/02-gateway/13-stream-plugin-dev.md#stage-2-tlsroute).

## Binding

Bind to different levels via annotations:

```yaml
# Gateway level — Pingora Service listeners (not UDP listeners)
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "my-stream-plugins"

# TCPRoute level
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "default/tcp-plugins"

# TLSRoute level
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "default/tls-plugins"

# UDPRoute level — Stage 1 only (no TLS handshake)
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "default/udp-plugins"
```

On `UDPRoute` the binding runs only the Stage-1 connection-context runtime
(`spec.plugins`, IP-based, evaluated at session creation, fail-close); it does
not use Pingora's listener `ConnectionFilter`, and `tlsRoutePlugins` never apply
because UDP has no TLS handshake. The F-016 session fast-path skips the route
pipeline for established sessions, so the check runs on the first packet of each
session (a denied client never establishes a session), matching TCP's
per-connection model. A Gateway-level attachment does not cover a UDP listener;
attach the resource to `UDPRoute` for UDP admission.

## Currently Supported Stream Plugins

| Type | Stage | Description |
|------|-------|-------------|
| `IpRestriction` | Stage 1 and TLS-route Stage 2 | Ordered `allowRefs`/`denyRefs` to central IpLists; no local CIDR collections or compilation. Optional unavailable references are omitted, with authored whitelist intent preserved. |
| `GlobalConnectionIpRestriction` | Stage 1 only | The plugin owns named reference-only `profiles` and a validated `activeProfile`. Optional `activeProfileRef` targets a Selector that names one local profile; each profile separately references IpLists. No geo block — combine with `GeoIpLocation` (below) as a separate, ordered Stage-1 entry instead. |
| `ConnectionRateLimit` | Stage 1 only | Limits new TCP/TLS connections or UDP session-admission attempts using the current rate configuration; it does not count packets on established UDP sessions. |
| `ConnectionLimit` | Stage 1 only | `maxConnections` (1..=10000000) caps concurrent connections in the attachment scope: live sockets per physical listener (Gateway, enforced before PROXY/TLS), connections in the TCP listener's route stage (TCPRoute), or sessions of the UDP listener (UDPRoute). Counts are owned by the listener (the route-stage count includes relays of routes that previously served the listener; a route on several listeners is capped per listener), are never shared between attachments of one resource, count all clients together (no per-client limit), and survive reloads; see `skills/04-review/listeners/regression-guards/downstream-connection-admission.md`. |
| `GeoIpLocation` | Stage 1 only | Same config shape as the HTTP `GeoIpLocation` plugin (`defaultAction`/`failOpen`/`denyRuleGroups`/`allowRuleGroups`), sharing its matching core; `source: RemoteIp` is rejected at L4 (no `X-Forwarded-For` to resolve). L4 configuration requires at least one allow or deny rule group because no L4 sink consumes resolved facts; rule-less `config: {}` is invalid and the generation fails closed. Denials from valid rules increment the bounded `edgion_geoip_denied_total{layer,reason}` metric. A Stage-1 listener denial is otherwise metric-only; TCP/UDP route denials retain their canonical protocol logs without GeoIP facts or sensitive denial reasons. See `docs/en/ops-guide/gateway/geoip.md`. |

Entries are flattened as `enable + type + config`; the older nested
`plugin: {type, config}` shape is not the current wire contract. Stage 2 admits
only `IpRestriction` in the current Rust enum.

## Differences from HTTP Plugins

| Dimension | HTTP Plugin (EdgionPlugins) | Stream Plugin (EdgionStreamPlugins) |
|-----------|------------------------------|--------------------------------------|
| Layer | L7 HTTP layer | L4 TCP/TLS connections and UDP sessions |
| Execution Position | ProxyHttp callback | Gateway listener `ConnectionFilter`, TCPRoute connection admission, UDPRoute session admission, or TLSRoute stage |
| Available Context | Full HTTP request | Stage-specific connection context (IP/port; TLSRoute also has SNI and mode-dependent TLS identity) |
| Failure Behavior | Return HTTP error | Close/reject a TCP/TLS connection or drop the UDP session-admission datagram |
| Binding | ExtensionRef Filter | Annotation |
| Hot Reload Source | ClientCache → ConfHandler | Annotation parsing → Store lookup |

## Status Schema

In Kubernetes, the native conditions below live in
`status.controllers[].status.conditions`, selected only by `controllerName`.
FileSystem/etcd keep the native shape; conf_sync carries complete source status
plus native `spec.currentStatus`. Reuse the shared 32-entry observation
[contract](../../01-architecture/01-controller/10-status-management.md#7-status-persistence).

```yaml
status:
  conditions:
    - type: Accepted
      status: "True"
      reason: Valid
```

## GatewayClass plugin execution policy

`EdgionGatewayConfig.spec.pluginPolicy` governs `stream/<Type>` connection entries and `tls-route/<Type>` TLSRoute entries.
The actual GatewayClass binding selects the policy, so shared plugin resources
can be allowed in one class and restricted in another. The Gateway publishes
compatible views dynamically; the Controller neither stamps entries nor emits
a global restriction condition on the shared plugin resource.

See [spec.pluginPolicy](../02-config/02-edgion-gateway-config.md#specpluginpolicy)
for allow/deny semantics, lifecycle boundaries and unavailable-policy behavior.

## Unknown config keys (`UnknownConfigFields` condition)

Ordinary advisory unknown-key handling follows `EdgionPlugins`: entry config
interiors preserve unknown fields, the Controller reports `UnknownConfigFields`,
and a clean apply removes that condition without changing Accepted.
Connection IpRestriction ignores HTTP-only `ipSource`, `message` and `status`:
valid `allowRefs`/`denyRefs` remain accepted with advisory unknown-key feedback.
These keys do not select a client identity or customize an HTTP response at L4.
Removed inline `allow`/`deny` is different: its presence is invalid even alongside
valid references and produces `Accepted=False` with reason `Invalid` and a
Gateway denial gate. Missing or malformed current policy also remains invalid.
GeoIpLocation is a fail-closed exception: config/group/rule unknown or removed
fields form a typed invalid generation, Accepted=False, and a live denial gate.
See `13-edgion-plugins.md` for the full
behavior description and `docs/en/ops-guide/plugin-config-unknown-fields.md` for the operator
guide (fleet-wide `GET /api/v1/diagnostics/unknown-fields`, and
`POST /api/v1/validate/{kind}` for a blocking gate of your own).

GeoIpLocation CIDR criteria contain ordered `ipRefs` only. All lists are required
before MMDB/failOpen/group shortcuts; content and availability remain live.
Unknown/removed input is transported as bounded invalidity, with static HTTP
configuration rejection / Stream denial and clean-generation recovery. Central
IpList partial-good compilation is intentional; empty available lists match none.
No Geo CIDR tree or CIDR expansion belongs to a consumer plugin.
