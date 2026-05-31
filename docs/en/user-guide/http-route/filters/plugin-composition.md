# Plugin Composition and Execution Order

When a request matches an HTTPRoute rule, Edgion runs filters and plugins from several sources across several proxy phases. This page describes exactly which plugin runs when, so you can reason about ordering, short-circuiting, and how Gateway API filters interact with `EdgionPlugins` referenced through `ExtensionRef`.

## Where plugins come from

A single request can pick up plugins from three sources:

| Source | How it is declared | Scope |
|--------|--------------------|-------|
| **Global plugins** | `EdgionGatewayConfig.spec.globalPluginsRef` (a list of `EdgionPlugins` references) | Every request handled by the gateway |
| **Route rule plugins** | `rules[].filters` on the HTTPRoute — both standard Gateway API filters and `ExtensionRef` entries | The matched rule |
| **BackendRef plugins** | `rules[].backendRefs[].filters` | The selected backend |

Each source contributes plugins to one or more execution stages. A standard Gateway API filter written directly in `rules[].filters` (for example `type: RequestHeaderModifier`) is mapped internally to the same plugin machinery as an `EdgionPlugins` plugin of the same kind, so the two families share one ordered pipeline.

## Execution stages

Edgion splits plugin execution into four stages that line up with the proxy lifecycle:

| Stage | When it runs | Sync / async |
|-------|--------------|--------------|
| **Request** | After the route matches, before the upstream is contacted | async |
| **Upstream response (header)** | When the upstream response headers arrive | sync |
| **Upstream response body** | For each body chunk streamed back from the upstream | sync |
| **Upstream response (async)** | Reserved async response hook (no built-in plugins map here today) | async |

Each plugin kind is valid only in specific stages. If you place a plugin in a stage it does not support, the route fails closed (see [Fail-closed behavior](#fail-closed-behavior)).

## Cross-source precedence

### Request stage

Request-stage plugins run in a fixed precedence across the three sources:

```text
global request plugins  →  route rule plugins  →  backendRef plugins
```

Each tier can short-circuit the request before the next tier runs. For example, a global IP-restriction plugin that rejects the client terminates the request before any route or backend plugin executes.

### Response stages

The response-header stage runs in this precedence:

```text
route rule response plugins  →  backendRef response plugins  →  global response plugins
```

This follows an onion model: the route and backend plugins that sit closest to the upstream run before the global plugins that wrap the whole gateway. Response-header operations that a request-stage plugin queued (for example a `set` from a `ResponseHeaderModifier` that was evaluated early) are drained first, so the response plugins observe them.

The response-body stage (used by streaming plugins such as `BandwidthLimit`) runs route rule plugins, then global plugins.

## Ordering within a single source

Within one source and one stage, plugins run in declaration order:

- Standard Gateway API filters run in the order they appear in `rules[].filters`.
- Plugins inside an `EdgionPlugins` resource run in the order they appear in `spec.requestPlugins`, `spec.upstreamResponseFilterPlugins`, `spec.upstreamResponseBodyFilterPlugins`, and `spec.upstreamResponsePlugins`.

## How Gateway API filters map to stages

When you write a standard filter in `rules[].filters`, Edgion routes it to the right stage automatically:

| Filter `type` | Stage(s) |
|---------------|----------|
| `RequestHeaderModifier` | Request |
| `RequestRedirect` | Request (terminating) |
| `URLRewrite` | Request |
| `RequestMirror` | Request (non-blocking) |
| `ResponseHeaderModifier` | Upstream response (header) |
| `ExtensionRef` | Request + upstream response (header) + upstream response body |

`ExtensionRef` is special: the referenced `EdgionPlugins` resource may declare plugins for any stage, so a single `ExtensionRef` entry is attached to the request, response-header, and response-body stages at once. At runtime each stage only runs the plugins from that resource that are valid for it.

> An `ExtensionRef` whose `kind` is a load-balancing policy is not a plugin — it is consumed by backend resolution and does not enter the plugin pipeline.

## ExtensionRef: pointing a route at EdgionPlugins

`ExtensionRef` is the standard Gateway API mechanism Edgion uses to attach its extension plugins to a route. It references one `EdgionPlugins` resource by name:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-guard
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-guard
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
    - type: RateLimit
      config:
        rate: 100
        burst: 50
```

### Reusing plugins across routes

The nested `type: ExtensionRef` form *inside* an `EdgionPlugins` resource (a plugin referencing another plugin) is not supported. Compose plugins in one of two ways instead:

- **Aggregate mode**: put several plugins in one `EdgionPlugins` resource; the route needs only a single `ExtensionRef`.
- **Side-by-side mode**: write several `ExtensionRef` entries in `rules[].filters`, each pointing to a different `EdgionPlugins` resource. They run in the order listed.

```yaml
filters:
  # Runs first
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: auth-plugins
  # Runs second
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: rate-limit-plugins
```

## Short-circuiting

Any request-stage plugin can terminate the request. When a plugin returns a terminating result (a redirect, an auth rejection, a mock response, or a gRPC error), the runtime stops the request-stage loop immediately, sets the response, and skips the remaining request-stage plugins. `RequestRedirect` is a typical terminating filter: it sends a 301/302 and the upstream is never contacted.

## Fail-closed behavior

If any plugin in a route has an invalid configuration, Edgion does **not** build that plugin and does **not** silently skip it. Instead the whole route fails closed: a single request-stage gate is prepended that rejects every matching request with `500` before the upstream is contacted. This guarantees a misconfigured security plugin can never be bypassed. The controller also surfaces the configuration error on the resource status.

## Related documents

- [Filters Overview](./overview.md)
- [HTTPRoute Overview](../overview.md)
- [Gateway API standard filters](./gateway-api/request-header-modifier.md)
- [Edgion extension plugins](./edgion-plugins/)
