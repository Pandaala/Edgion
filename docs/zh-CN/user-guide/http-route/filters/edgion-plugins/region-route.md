# RegionRoute（HTTPRoute 过滤器）

> **🔌 Edgion 扩展**
>  
> RegionRoute 是 `EdgionPlugins` CRD 提供的多区域流量路由插件，不属于标准 Gateway API。

RegionRoute 可以作为 HTTPRoute 的过滤器使用，也可以通过 `EdgionPlugins` 全局配置。本页介绍如何在 HTTPRoute 中使用；完整配置参数请见 [独立插件页](../../edgion-plugins/region-route.md)。

## 在 HTTPRoute 中使用

### 最简配置

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

完整的 `routeRules` 类型（`RouteByDirectRegion`、`RouteByKeyConfMatch`、`RouteByHashRange`）、`hashCalc`、`autoFailover` 等参数请见独立插件页。

## 相关文档

- [RegionRoute 完整文档（包含所有配置参数）](../../edgion-plugins/region-route.md)
- [插件组合与引用](../plugin-composition.md)
- [过滤器总览](../overview.md)
