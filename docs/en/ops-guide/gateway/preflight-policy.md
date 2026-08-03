# Preflight Policy

How Edgion handles CORS preflight (`OPTIONS`) requests at the gateway.

> **🔌 Edgion extension**
>
> The preflight policy is configured through the `EdgionGatewayConfig` CRD (`spec.preflightPolicy`); it is not part of standard Gateway API.

## Overview

Browsers send a CORS preflight `OPTIONS` request before certain cross-origin calls. Edgion
can answer these preflights at the edge so they do not have to reach every backend. By default,
a standard preflight is handled by the route's CORS filter when one is configured; otherwise
Edgion replies `204 No Content`. A CORS filter may deliberately pass the request upstream, as
described in [Relationship with CORS](#relationship-with-cors).

You only need `preflightPolicy` if you want to change *which* `OPTIONS` requests are treated
as preflight, or the default status code. It is set on the `EdgionGatewayConfig` referenced by
your GatewayClass — not via an annotation.

## Configuration

`preflightPolicy` is a field of `EdgionGatewayConfig.spec`. Reference the config from a
GatewayClass via `parametersRef`:

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
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  preflightPolicy:
    # Which OPTIONS requests count as preflight
    mode: cors-standard
    # Status code used when no CORS plugin is attached to the route
    statusCode: 204
```

## Field reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | Enum | `cors-standard` | Which `OPTIONS` requests are treated as preflight. Allowed values: `cors-standard` \| `all-options` |
| `statusCode` | Integer | `204` | Preflight response status code used when no CORS plugin is attached to the route |

### `mode` values

| Value | Meaning |
|-------|---------|
| `cors-standard` | Strict CORS detection: the request must be `OPTIONS` and carry both `Origin` and `Access-Control-Request-Method`. Standard browser behavior. |
| `all-options` | Select every `OPTIONS` request for global preflight handling, without first requiring CORS headers. A route-level CORS filter still decides whether it owns the selected request. |

## Default behavior

Edgion handles preflight automatically; the default `preflightPolicy` is `mode: cors-standard`
with `statusCode: 204`. If a route references a CORS plugin, the preflight response follows the
CORS rules, including `preflightContinue`; otherwise Edgion returns the configured `statusCode`.

## Scenarios

### Use the default (no configuration)

For most deployments you do not configure `preflightPolicy` at all. Standard browser CORS
preflights are detected and answered automatically.

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  # No preflightPolicy block — defaults apply (mode cors-standard, statusCode 204)
  server:
    threads: 0
```

### Treat all OPTIONS as preflight

Use `all-options` when some clients send `OPTIONS` without standard CORS headers and routes
without a CORS filter should answer those requests locally. On a route with a CORS filter, an
`OPTIONS` request without `Origin` is not a CORS request and continues upstream.

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  preflightPolicy:
    mode: all-options
```

### Customize the default status code

Reply with `200` instead of `204` for clients that mishandle empty `204` responses.

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  preflightPolicy:
    mode: cors-standard
    statusCode: 200
```

## Relationship with CORS

How preflight interacts with CORS depends on how CORS is configured on the route:

- **Gateway API native `type: CORS` filter** — the preflight handler does **not** intervene;
  preflight is handled inside the route-level filter chain. `preflightPolicy` has no effect on
  this path.
- **EdgionPlugins CORS plugin (via `ExtensionRef`)** — the preflight handler delegates to the
  normal route plugin pipeline. For an allowed origin, a standard preflight is answered locally
  with the configured CORS headers unless `preflightContinue: true`; that setting forwards the
  request upstream. A denied origin receives a local `204` without CORS headers regardless of
  `preflightContinue`. An `OPTIONS` request without `Origin` is also forwarded because the CORS
  plugin does not own non-CORS requests, even when `mode: all-options` selected it for global
  handling. Entry-level plugin conditions remain effective; when they do not match, the CORS
  plugin is skipped and the request continues upstream.
- **No route-level CORS filter** — a request selected by `preflightPolicy` is answered locally
  with `statusCode` and no CORS headers.

The resulting behavior is:

| Request | Route CORS filter | Result |
|---------|-------------------|--------|
| Standard preflight (`OPTIONS` + `Origin` + `Access-Control-Request-Method`) | None | Local `statusCode`, without CORS headers |
| Standard preflight, allowed origin | Native Gateway API CORS | Local `204`, with CORS headers |
| Standard preflight, denied origin | Native Gateway API CORS | Local `204`, without CORS headers |
| Standard preflight, allowed origin | EdgionPlugins CORS, `preflightContinue: false` | Local `204`, with CORS headers |
| Standard preflight, allowed origin | EdgionPlugins CORS, `preflightContinue: true` | Forwarded upstream; CORS headers are applied to the upstream response |
| Standard preflight, denied origin | EdgionPlugins CORS, either `preflightContinue` value | Local `204`, without CORS headers |
| Standard preflight | EdgionPlugins CORS whose entry conditions do not match | Forwarded upstream without CORS headers |
| `OPTIONS` without `Origin`, `mode: all-options` | None | Local `statusCode`, without CORS headers |
| `OPTIONS` without `Origin`, `mode: all-options` | Native or EdgionPlugins CORS | Forwarded upstream as a non-CORS request |
| `OPTIONS` with `Origin` but without `Access-Control-Request-Method`, `mode: all-options` | None | Local `statusCode`, without CORS headers |
| `OPTIONS` with `Origin` but without `Access-Control-Request-Method`, `mode: all-options` | Native or EdgionPlugins CORS | Forwarded as an actual CORS request; an allowed origin receives CORS headers on the upstream response |

## Edge cases and caveats

- `preflightPolicy` lives in `EdgionGatewayConfig`, not in a Gateway annotation. Setting it on
  a Gateway has no effect.
- The policy applies to the entire GatewayClass that references the `EdgionGatewayConfig`, not
  to a single Gateway or listener.
- This policy controls only the `OPTIONS` short-circuit. It does not add `Access-Control-Allow-*`
  headers to actual cross-origin responses; a CORS plugin or the backend remains responsible
  for those.
- Changes apply dynamically when the `EdgionGatewayConfig` is updated — no Gateway restart.

## Related documents

- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [GatewayClass](gateway-class.md)
- [CORS plugin](../../user-guide/http-route/filters/edgion-plugins/cors.md)
- [Gateway overview](overview.md)
