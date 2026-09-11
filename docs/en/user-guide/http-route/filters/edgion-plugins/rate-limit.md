# RateLimit (HTTPRoute Filter)

> **🔌 Edgion extension**
> 
> RateLimit is a request rate limiting plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

RateLimit can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page covers usage within HTTPRoute; for full configuration parameters, see the [standalone plugin page](../../../edgion-plugins/rate-limit.md).

## Using in HTTPRoute

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
            name: api-rate-limit
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-rate-limit
  namespace: production
spec:
  requestPlugins:
    - type: RateLimit
      config:
        # Allowed requests per interval
        rate: 100
        interval: "60s"
        key:
          - type: remoteIp
```

> **Schema note**: RateLimit runs in the `requestPlugins` phase and uses the
> `type` / `config` plugin-entry form. The fields are `rate` (max requests per
> interval) and `interval` (the time window) — there are no `limit` / `window`
> fields. See the [standalone plugin page](../../../edgion-plugins/rate-limit.md)
> for the full parameter table.

## Related Links

For local passes that avoid Redis and excess requests that use shared supplemental
allowance, configure `onLimitExceeded: Continue` and the
[Redis overflow condition](./rate-limit-redis.md#local-allowance-before-redis-overflow-admission).

- [RateLimit Standalone Plugin Page (full configuration)](../../../edgion-plugins/rate-limit.md)
- [RateLimitRedis (Redis-backed precise cluster rate limiting)](./rate-limit-redis.md)
- [IP Restriction](./ip-restriction.md)
- [Plugin Composition](../plugin-composition.md)
