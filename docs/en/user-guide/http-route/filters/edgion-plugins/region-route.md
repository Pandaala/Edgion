# RegionRoute (HTTPRoute Filter)

> **🔌 Edgion Extension**
>  
> RegionRoute is a multi-region traffic routing plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

RegionRoute can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page describes how to use it within HTTPRoute; for complete configuration parameters, see the [standalone plugin page](../../edgion-plugins/region-route.md).

## Using in HTTPRoute

### Minimal Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
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
            name: my-region-route-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        regions:
          - name: us-east
            hashRange: [0, 499]
            backendEndpoint: us-east.example.com:8443
            tls: true
          - name: us-west
            hashRange: [500, 999]
            backendEndpoint: us-west.example.com:8443
            tls: true
        keyGet:
          - type: header
            name: X-Account-Id
        routeRules:
          - type: RouteByHashRange
```

For complete `routeRules` types (`RouteByDirectRegion`, `RouteByKeyConfMatch`, `RouteByHashRange`), `hashCalc`, `autoFailover`, and other parameters, see the standalone plugin page.

## Related Documentation

- [RegionRoute Full Documentation (all configuration parameters)](../../edgion-plugins/region-route.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)
