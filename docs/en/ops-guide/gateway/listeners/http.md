# HTTP Listener

Configure an HTTP protocol listener.

## Basic Configuration

```yaml
listeners:
  - name: http
    port: 80
    protocol: HTTP
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| name | string | Yes | | Listener name |
| port | int | Yes | | Listening port |
| protocol | string | Yes | | Protocol (HTTP) |
| hostname | string | | | Hostname filter |
| allowedRoutes | object | | | Allowed routes |

## Hostname Filtering

Restrict this listener to handle only specific domains:

```yaml
listeners:
  - name: api
    port: 80
    protocol: HTTP
    hostname: "api.example.com"
```

## Route Binding Control

### Allow All Namespaces

```yaml
allowedRoutes:
  namespaces:
    from: All
```

### Allow Same Namespace Only

```yaml
allowedRoutes:
  namespaces:
    from: Same
```

### Allow Specific Namespaces

```yaml
allowedRoutes:
  namespaces:
    from: Selector
    selector:
      matchLabels:
        env: production
```

## Examples

### Example 1: Multi-Port Listening

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: multi-port
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
    - name: http-alt
      port: 8080
      protocol: HTTP
```

### Example 2: Domain-Based Separation

```yaml
listeners:
  - name: api
    port: 80
    protocol: HTTP
    hostname: "api.example.com"
    allowedRoutes:
      kinds:
        - kind: HTTPRoute
  - name: web
    port: 80
    protocol: HTTP
    hostname: "www.example.com"
    allowedRoutes:
      kinds:
        - kind: HTTPRoute
```

## Related Documentation

- [HTTPS Listener](./https.md)
- [TCP Listener](./tcp.md)
