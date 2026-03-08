# Gateway Resource Overview

Gateway is the core resource of the Gateway API, defining traffic entry points.

## Resource Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: default
spec:
  gatewayClassName: edgion  # References a GatewayClass
  listeners:                 # Listener list
    - name: http
      port: 80
      protocol: HTTP
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        mode: Terminate
        certificateRefs:
          - name: tls-secret
```

## Core Concepts

### GatewayClass

A Gateway must reference a GatewayClass, which defines the implementation of the Gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

### Listeners

Each listener defines a traffic entry point:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | string | Yes | Listener name (routes bind via this name) |
| port | int | Yes | Listening port |
| protocol | string | Yes | Protocol (HTTP/HTTPS/TCP/TLS) |
| hostname | string | | Hostname to match |
| tls | object | | TLS configuration (required for HTTPS/TLS) |
| allowedRoutes | object | | Allowed route bindings |

## Examples

### Example 1: HTTP Gateway

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: http-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: All
```

### Example 2: HTTPS Gateway

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: https-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-tls
      allowedRoutes:
        namespaces:
          from: All
```

## Related Documentation

- [GatewayClass Configuration](./gateway-class.md)
- [Listener Configuration](./listeners/)
- [TLS Configuration](./tls/)
