# GRPCRoute Filters

GRPCRoute supports a subset of the filters available to HTTPRoute. Filters run in the order they are declared and can modify metadata, mirror traffic, or invoke Edgion plugins.

## Supported filters

| Filter | Description |
|--------|-------------|
| `RequestHeaderModifier` | Add, set, or remove gRPC request metadata (headers) |
| `ResponseHeaderModifier` | Add, set, or remove response metadata |
| `RequestMirror` | Mirror calls to a second backend without affecting the client response |
| `ExtensionRef` | Invoke an Edgion plugin via the `EdgionPlugins` CRD |

gRPC does not support the `RequestRedirect` or `URLRewrite` filters — they are HTTP-only.

## Modifying metadata

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: user-service-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-internal-caller
                value: gateway
            remove:
              - x-debug
      backendRefs:
        - name: user-service
          port: 9000
```

## Mirroring traffic

Send a copy of matched calls to a second backend (for example, to validate a new service version) while the client still receives the response from the primary backend:

```yaml
spec:
  rules:
    - filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: user-service-shadow
              port: 9000
      backendRefs:
        - name: user-service
          port: 9000
```

## Invoking an Edgion plugin

Reference an `EdgionPlugins` resource with `ExtensionRef` to apply gateway plugins (for example authentication or rate limiting) to gRPC calls:

```yaml
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: grpc-auth
      backendRefs:
        - name: user-service
          port: 9000
```

See the [Edgion plugin catalog](../../http-route/filters/edgion-plugins/README.md) for the available plugins; note that request-body-dependent plugins may behave differently for streaming RPCs.

An `EdgionPlugins` resource referenced from `backendRefs[].filters` must not contain an enabled
`DirectEndpoint`, `DynamicInternalUpstream`, `DynamicExternalUpstream`, `RegionRoute`, or `Canary`
request plugin. BackendRef filters run after backend selection. The Controller marks the rule
invalid, and the Gateway rejects the reference if it reaches runtime.

## Related documents

- [GRPCRoute Overview](../overview.md)
- [GRPCRoute Matching Rules](../matches/overview.md)
- [GRPCRoute Backends](../backends/overview.md)
