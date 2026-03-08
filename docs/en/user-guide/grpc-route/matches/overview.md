# GRPCRoute Match Rules

GRPCRoute supports matching requests by hostname, gRPC method, and other dimensions.

## Common Match Dimensions

- Hostname: Corresponds to TLS SNI/Host.
- Method: Exact match by `service`/`method`.
- Header (if supported by implementation): Extended matching by metadata.

## Example: Matching by Service and Method

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: billing-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
  rules:
    - matches:
        - method:
            service: billing.v1.BillingService
            method: CreateInvoice
      backendRefs:
        - name: billing-v1
          port: 50051
```

## Best Practices

1. Prefer exact matching with service + method to reduce ambiguity.
2. Place higher-priority rules first to avoid being consumed by wildcard rules.
