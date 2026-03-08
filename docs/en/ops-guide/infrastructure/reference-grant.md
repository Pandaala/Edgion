# Cross-Namespace Reference (ReferenceGrant)

`ReferenceGrant` is used to explicitly authorize cross-namespace resource references, avoiding security risks from unrestricted access.

## Typical Scenarios

- Gateway is in `gateway-system`, certificate Secret is in the `security` namespace.
- Route is in a business namespace, backend Service is in a shared namespace.

## Example: Allow Gateway to Reference Cross-Namespace Secret

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-gw-to-secret
  namespace: security
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: Gateway
      namespace: gateway-system
  to:
    - group: ""
      kind: Secret
```

## Security Recommendations

1. Keep `from` granularity as small as possible, specifying namespace and kind precisely.
2. Only include necessary kinds in `to`.
3. Regularly audit cross-namespace authorization objects.

## Related Documentation

- [Secret Management](./secret-management.md)
- [mTLS Configuration](./mtls.md)
- [GatewayClass Configuration](../gateway/gateway-class.md)
