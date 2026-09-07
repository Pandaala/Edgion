---
name: stream-plugins-features
description: EdgionStreamPlugins CRD schema: two-stage plugins at the TCP/TLS layer.
---

# EdgionStreamPlugins — Stream Plugins

> API: `edgion.io/v1` | Scope: Namespaced

EdgionStreamPlugins defines plugins at the TCP/TLS layer, executed during Pingora's ConnectionFilter stage, before HTTP parsing.

## Full Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: my-stream-plugins
  namespace: default
spec:
  # Stage 1: ConnectionFilter (pre-TLS, only IP information available)
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

  # Stage 2: TlsRoute (post-TLS handshake, SNI/certificate information available)
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
| Stage 1: ConnectionFilter | After TCP connection is established, before TLS handshake | Source IP, destination port | `spec.plugins` |
| Stage 2: TlsRoute | After TLS handshake completes | SNI, client certificate, IP | `spec.tlsRoutePlugins` |

## Binding

Bind to different levels via annotations:

```yaml
# Gateway level — all connections
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

On `UDPRoute` the binding runs only the Stage-1 ConnectionFilter (`spec.plugins`, IP-based, evaluated at session creation, fail-close); `tlsRoutePlugins` never apply because UDP has no TLS handshake. The F-016 session fast-path skips the route pipeline for established sessions, so the check runs on the first packet of each session (a denied client never establishes a session), matching TCP's per-connection model.

## Currently Supported Stream Plugins

| Type | Stage | Description |
|------|-------|-------------|
| `IpRestriction` | Stage 1 + Stage 2 | IP allow/deny list (CIDR supported); rules may be inline named `IpGroup` entries under `allow`/`deny` or referenced from a central `IpList` EdgionConfigData via `allowRefs`/`denyRefs` (merged into the same pools; a missing/disabled referenced IpList causes allowRef to fail closed). Inline `allow`/`deny` groups are **partial-good**: a bad CIDR entry is skipped (not fatal); the plugin loads with valid entries. Bad entries surface as `PartiallyInvalid` status (reason `RuleInvalid`; `Accepted` stays `True`). A skipped inline deny entry increments `edgion_ip_deny_entries_skipped_total{scope="stream_l4"|"tls_route"}`. |
| `GlobalConnectionIpRestriction` | Stage 1 only | EdgionConfigData-referenced IP restriction. The plugin config holds `overrideRef`; referenced profiles carry the rule sets. No geo block — combine with `GeoIpLocation` (below) as a separate, ordered Stage-1 entry instead. |
| `ConnectionRateLimit` | Stage 1 only | Limits new connections before TLS/application processing using the current connection-rate configuration. |
| `GeoIpLocation` | Stage 1 only | Same config shape as the HTTP `GeoIpLocation` plugin (`defaultAction`/`failOpen`/`denyRuleGroups`/`allowRuleGroups`), sharing its matching core; `source: RemoteIp` is rejected at L4 (no `X-Forwarded-For` to resolve). Parse-only mode (`config: {}`) is a true no-op at L4 — there is no per-connection access log to populate. Denials are counted via `edgion_geoip_denied_total{layer,reason}`, never logged. See `docs/en/ops-guide/gateway/geoip.md`. |

Entries are flattened as `enable + type + config`; the older nested
`plugin: {type, config}` shape is not the current wire contract. Stage 2 admits
only `IpRestriction` in the current Rust enum.

## Differences from HTTP Plugins

| Dimension | HTTP Plugin (EdgionPlugins) | Stream Plugin (EdgionStreamPlugins) |
|-----------|------------------------------|--------------------------------------|
| Layer | L7 HTTP layer | L4 TCP/TLS layer |
| Execution Position | ProxyHttp callback | ConnectionFilter |
| Available Context | Full HTTP request | Connection information only (IP, SNI) |
| Failure Behavior | Return HTTP error | Close connection |
| Binding | ExtensionRef Filter | Annotation |
| Hot Reload Source | ClientCache → ConfHandler | Annotation parsing → Store lookup |

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted
      status: "True"
      reason: Valid
```

## Controller Plugin Policy (Allowlist / Denylist)

An operator can restrict which `stream/<Type>` (connection stage) and
`tls-route/<Type>` (TLS-route stage) plugin types the Gateway is allowed
to instantiate via the Controller's optional `validation.plugin_policy`
config section (absent by default = allow all). A restricted entry is
stamped, not force-disabled, and is reported via a `PluginPolicyRestricted`
status condition; on the data plane a Block closes the connection (the
configured status is ignored) and a Bypass skips the plugin. See
[validation.plugin_policy](../02-config/00-controller-config.md#validationplugin_policy)
for the config schema and `docs/en/ops-guide/plugin-policy.md` for the
operator guide.

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
