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
kind: EdgionStreamPlugins
metadata:
  name: my-stream-plugins
  namespace: default
spec:
  # Stage 1: connection context (IP/port information only)
  plugins:
    - enable: true
      type: IpRestriction
      config:
        allow:
          - name: internal-networks
            description: Private client networks
            cidrs:
              - "10.0.0.0/8"
              - "192.168.1.0/24"
        deny:
          - name: blocked-hosts
            cidrs:
              - "10.0.0.100"
          # allowRefs/denyRefs (optional): reference central IpList EdgionConfigData objects;
          # merged into the same allow/deny pools as inline entries.
          # namespace defaults to this EdgionStreamPlugins resource's namespace if omitted.
        allowRefs:
          - name: corp-allowlist   # references an IpList EdgionConfigData named "corp-allowlist"

  # Stage 2: TlsRoute (post-route-match; TLS identity only after termination)
  tlsRoutePlugins:
    - enable: true
      type: IpRestriction
      config:
        allow:
          - name: tls-clients
            cidrs:
              - "172.16.0.0/12"
        # allowRefs/denyRefs also available here (same semantics as Stage 1)
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
| `IpRestriction` | Stage 1 + Stage 2 | IP allow/deny list (CIDR supported); rules may be inline named `IpGroup` entries under `allow`/`deny` or referenced from a central `IpList` EdgionConfigData via `allowRefs`/`denyRefs` (merged into the same pools; a missing/disabled referenced IpList causes allowRef to fail closed). Inline `allow`/`deny` groups are **partial-good**: a bad CIDR entry is skipped (not fatal); the plugin loads with valid entries. Bad entries surface as `PartiallyInvalid` status (reason `RuleInvalid`; `Accepted` stays `True`). A skipped inline deny entry increments `edgion_ip_deny_entries_skipped_total{scope="stream_l4"|"tls_route"}`. |
| `GlobalConnectionIpRestriction` | Stage 1 only | EdgionConfigData-referenced IP restriction. The plugin config holds `overrideRef`; referenced profiles carry the rule sets. No geo block — combine with `GeoIpLocation` (below) as a separate, ordered Stage-1 entry instead. |
| `ConnectionRateLimit` | Stage 1 only | Limits new TCP/TLS connections or UDP session-admission attempts using the current rate configuration; it does not count packets on established UDP sessions. |
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

Same mechanism and same condition as `EdgionPlugins` (13-edgion-plugins.md) — `spec.plugins` /
`spec.tlsRoutePlugins` entry `config` blocks are `x-kubernetes-preserve-unknown-fields`, so a
mistyped key (e.g. a `GeoIpLocation` config typo) is pruned by nothing and silently dropped
by serde. The Controller reports it via a dedicated `UnknownConfigFields` condition
(`status: "True"`, `reason: UnknownFieldsDetected`, message = the full path list) plus a WARN
log line; `Accepted` and the abnormal-resource metric are untouched, and the condition is
removed on the next apply once the key is fixed. See `13-edgion-plugins.md` for the full
behavior description and `docs/en/ops-guide/plugin-config-unknown-fields.md` for the operator
guide (fleet-wide `GET /api/v1/diagnostics/unknown-fields`, and
`POST /api/v1/validate/{kind}` for a blocking gate of your own).
