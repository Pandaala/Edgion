# Preflight Policy

How Edgion handles CORS preflight (`OPTIONS`) requests at the gateway.

> **🔌 Edgion extension**
>
> The preflight policy is configured through the `EdgionGatewayConfig` CRD (`spec.preflightPolicy`); it is not part of standard Gateway API.

## Overview

Browsers send a CORS preflight `OPTIONS` request before certain cross-origin calls. Edgion
answers these preflights at the edge so they do not have to reach every backend. By default
this works without any configuration: if the route has a CORS plugin, Edgion replies using
that CORS configuration; otherwise it replies `204 No Content`.

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
| `all-options` | Treat every `OPTIONS` request as preflight, without checking CORS headers. For non-standard clients that omit CORS headers. |

## Default behavior

Edgion handles preflight automatically; the default `preflightPolicy` is `mode: cors-standard`
with `statusCode: 204`. If a route references a CORS plugin, the preflight response follows the
CORS rules; otherwise Edgion returns the configured `statusCode`.

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

Use `all-options` when some clients send `OPTIONS` without standard CORS headers.

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
- **EdgionPlugins CORS plugin (via `ExtensionRef`)** — the preflight handler runs first,
  finds the route's CORS configuration, and answers using it. If no CORS configuration is
  found, it returns the configured `statusCode`.

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
