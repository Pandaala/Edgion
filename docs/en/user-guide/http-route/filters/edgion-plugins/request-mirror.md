# Request Mirror Plugin

> **🔌 Edgion extension**
> 
> Edgion supports both the standard Gateway API `RequestMirror` filter and the reusable `EdgionPlugins` `RequestMirror` configuration.
> This page focuses on the reusable `EdgionPlugins` form and the shared runtime behavior behind both entry points.

## Overview

Request Mirror asynchronously mirrors inbound requests to another backend service without affecting the primary request processing. Useful for traffic replication, canary testing, and request auditing.

**Features**:
- Asynchronous mirroring, does not block primary request
- Supports fractional traffic sampling
- Mirror results can be recorded in access logs
- Concurrency limits to prevent mirror target overload

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mirror-plugin
spec:
  requestPlugins:
      type: RequestMirror
      config:
        backendRef:
          name: mirror-service
          port: 8080
```

---

## Cross-namespace authorization

The default target namespace is the declaring **EdgionPlugins** resource's
namespace. Only core Service targets (`group: ""`, `kind: Service`) are supported.

With Controller ReferenceGrant validation enabled (the default), a cross-namespace
mirror requires a grant in the target namespace. Its `from` must identify
`group: edgion.io`, `kind: EdgionPlugins`, and the plugin resource's namespace.
A grant for HTTPRoute does not authorize this plugin reference.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-production-plugin-mirror
  namespace: testing
spec:
  from:
    - group: edgion.io
      kind: EdgionPlugins
      namespace: production
  to:
    - group: ""
      kind: Service
      name: mirror-service
```

A denied target sets `ResolvedRefs=False/RefNotPermitted` on EdgionPlugins.
Edgion skips that mirror with `mirror ref denied` in the request's plugin log;
the primary request continues. Unsupported target groups or kinds are skipped
and reported as `InvalidKind`. Grant changes automatically revalidate the plugin.
New requests use the updated decision after configuration sync; already-started
mirror tasks are not cancelled by revocation.

If either the previous or incoming plugin configuration contains an enabled
cross-namespace mirror permitted by the current GatewayClass
`EdgionGatewayConfig.spec.pluginPolicy`, an invalid DSL/WAF
update fails closed with HTTP 500 instead of retaining the previous plugin runtime.
This prevents an old mirror from retaining authorization after its target is
changed, removed, or revoked. After the configuration is valid again, ordinary
mirror denial only skips the mirror.

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `backendRef` | Object | **Required** | Mirror backend reference |
| `backendRef.name` | String | **Required** | Backend service name |
| `backendRef.group` | String | `""` (core) | API group of the referent |
| `backendRef.kind` | String | `Service` | Kind of the referent |
| `backendRef.namespace` | String | same as EdgionPlugins | Backend service namespace |
| `backendRef.port` | Integer | none | Backend service port |
| `fraction` | Object | none (100%) | Mirror traffic fraction (Gateway API style) |
| `fraction.numerator` | Integer | **Required** (within `fraction`) | Numerator |
| `fraction.denominator` | Integer | `100` | Denominator |
| `percent` | Integer | none (100%) | Percentage of requests to mirror (0–100). Gateway API v1.5 field name |
| `connectTimeoutMs` | Integer | `1000` | Connection timeout (ms) |
| `writeTimeoutMs` | Integer | `1000` | Write timeout (ms) |
| `maxBufferedChunks` | Integer | `5` | Maximum buffered body chunks per mirror, `1..=1024` |
| `maxBufferedBytes` | String | `1MiB` | Maximum retained body bytes per mirror, `1B..=1GiB` |
| `mirrorLog` | Boolean | `true` | Emit a dedicated mirror line in the access log |
| `maxConcurrent` | Integer | `1024` | Maximum concurrent tasks per plugin instance, `1..=65536`, subject to the shared process limit |
| `channelFullTimeoutMs` | Integer | `0` | Max ms to wait for mirror channel space before abandoning the mirror. `0` = abandon immediately (zero impact on the main request) |

Invalid capacities, including zero, are configuration errors in EdgionPlugins.
The equivalent HTTPRoute annotations log invalid values at configuration loading
and use the corresponding field defaults.

All instances also share the Gateway process budgets: `request_mirror.max_concurrent`
(default `1024`) and `request_mirror.max_buffered_bytes` (default `64MiB`). Configure
them in the [Gateway configuration file](../../../../ops-guide/configuration/gateway-config.md#request_mirror).
These settings take effect on process restart; route/plugin limits apply to new
requests after configuration reload. Budget exhaustion skips or cancels the mirror
copy and the primary request continues. The byte budget covers retained mirror body
data, not total process RSS or connection/TLS overhead.

For an already buffered request, mirroring reads the committed body snapshot with
an independent cursor. Its retained memory is charged to the process byte budget;
`maxBufferedBytes` bounds outstanding streamed chunks, not the snapshot's total
length. A disk-backed snapshot remains streamed. Operators can raise or lower the
process budgets; raising the byte limit deliberately permits more retained memory.

---

## Usage Scenarios

### Scenario 1: Full Mirror to Test Service

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: test-service
        namespace: testing
        port: 8080
      mirrorLog: true
```

### Scenario 2: Fractional Traffic Sampling

Mirror only 10% of traffic:

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: analytics-service
        port: 8080
      fraction:
        numerator: 1
        denominator: 10
```

### Scenario 3: Percentage-Based Sampling

`percent` is an alternative to `fraction` (Gateway API v1.5 style). Mirror 5% of traffic:

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: analytics-service
        port: 8080
      # Equivalent to fraction: { numerator: 5, denominator: 100 }
      percent: 5
```

### Scenario 4: Concurrency Limiting

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: mirror-service
        port: 8080
      maxConcurrent: 50
      connectTimeoutMs: 500
      writeTimeoutMs: 500
```

---

## Important Notes

1. Mirror delivery is asynchronous; runtime overload does not change the primary response status. Copying admitted body data adds bounded work, and an explicitly configured queue wait can add latency
2. Mirror failures do not cause the primary request to fail
3. Request body is also mirrored; `maxBufferedChunks` and `maxBufferedBytes` independently bound retained stream data. These are buffering limits, not cumulative upload size limits
4. When concurrent mirrors reach `maxConcurrent`, new mirror requests are dropped
5. `channelFullTimeoutMs: 0` (default) abandons the mirror immediately when its body queue is full; a small positive value (100–500ms) grants a brief back-pressure window before abandoning. Byte-budget admission never waits
6. If the downstream transport does not reach its real end-of-stream before mirroring stops, Edgion cancels the mirror request rather than letting the mirror target accept a partial upload as complete; the mirror outcome is `body_abandoned`. This covers both the primary upstream stopping body consumption and a later request-stage plugin ending the request before the body pump runs. Only a transport-level end-of-stream on the request headers proves that an empty body is already complete; a `Content-Length: 0` declaration alone does not provide that guarantee on HTTP/2

---

## FAQ

### Q1: What is the difference between `fraction` and `percent`?

**A**: They are two ways to express the same sampling ratio. `fraction` uses `numerator` / `denominator` (Gateway API classic style); `percent` is an integer 0–100 (Gateway API v1.5 style). Use whichever is more convenient — set only one.

### Q2: Can a slow mirror backend slow down my real traffic?

**A**: No, by default. With `channelFullTimeoutMs: 0` the gateway abandons the mirror the moment its buffer is full rather than waiting. If you want to give a briefly-lagging mirror a chance to catch up, set `channelFullTimeoutMs` to a small value (e.g. `200`); beyond that window the mirror is dropped and the main request proceeds normally.

### Q3: Is this the same as the Gateway API `RequestMirror` filter?

**A**: Yes — both entry points share the same runtime. You can configure mirroring inline via the standard Gateway API `RequestMirror` filter, or as a reusable `EdgionPlugins` `RequestMirror` entry referenced through `ExtensionRef`. This page documents the reusable form.

---

## Related Docs

- [ProxyRewrite](./proxy-rewrite.md)
- [Filters Overview](../overview.md)
