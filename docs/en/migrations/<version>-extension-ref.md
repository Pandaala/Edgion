# Migration Guide: Nested ExtensionRef Removal

**Version:** `<version>`

## Background

The `EdgionPlugin::ExtensionRef` variant has been removed. `EdgionPlugins.spec.requestPlugins / upstreamResponseFilterPlugins / upstreamResponseBodyFilterPlugins / upstreamResponsePlugins` no longer accept `type: ExtensionRef`. The Gateway API standard `ExtensionRef` on `HTTPRoute` / `GRPCRoute` filter lists is preserved, and so is `ExtensionRef` on `HTTPBackendRef.filters`.

## Example 1: nested usage → aggregate EdgionPlugins

**Before (no longer supported):** A's `requestPlugins` references B by nested `ExtensionRef`:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  namespace: ns
  name: B
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
    - type: RealIp
      config:
        trustedAddresses: ["10.0.0.0/8"]
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  namespace: ns
  name: A
spec:
  requestPlugins:
    - type: ExtensionRef
      config:
        name: B
    - type: BasicAuth
      config:
        secretRef:
          name: basic-auth-users
```

**After (recommended):** Inline B's `requestPlugins` directly into A:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  namespace: ns
  name: A
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
    - type: RealIp
      config:
        trustedAddresses: ["10.0.0.0/8"]
    - type: BasicAuth
      config:
        secretRef:
          name: basic-auth-users
```

## Example 2: reuse one plugin chain across multiple HTTPRoutes

**Before (nested, no longer supported):** A references B; multiple routes reference A so they share B's logic:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata: { namespace: ns, name: A }
spec:
  requestPlugins:
    - type: ExtensionRef
      config:
        name: B
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata: { namespace: ns, name: route-1 }
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            kind: EdgionPlugins
            name: A
```

**After (recommended):** Either have multiple routes reference the same `EdgionPlugins` directly, or list multiple `ExtensionRef` entries side-by-side in the route's filter list:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  namespace: ns
  name: route-1
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            kind: EdgionPlugins
            name: shared-bundle-1
        - type: ExtensionRef
          extensionRef:
            kind: EdgionPlugins
            name: shared-bundle-2
```

## CRD field removal

The `HTTPRouteFilter.extensionRefMaxDepth` and `GRPCRouteFilter.extensionRefMaxDepth` fields have been deleted. If existing yaml carries these fields, strip them — otherwise Kubernetes will reject the resource with an unknown-field error.

## Design rationale

Align with the Envoy filter chain model: filter chains are flat lists, not graphs. Reuse happens at the configuration layer ("multiple routes referencing the same resource", paralleling Envoy ECDS), not at the behavior layer ("plugins referencing plugins").
