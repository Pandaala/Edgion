# GatewayClass Configuration

GatewayClass defines the implementation type of a Gateway, similar to IngressClass.

> **🔌 Edgion Extension**
> 
> `parametersRef` can reference the `EdgionGatewayConfig` CRD for advanced configuration. This is an Edgion extension feature.

## Resource Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:              # Optional: reference configuration parameters
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config
```

## Configuration Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| controllerName | string | Yes | Controller identifier |
| parametersRef | object | | Configuration parameter reference |
| description | string | | Description |

## Edgion Controller Name

The controller name used by Edgion:

```yaml
controllerName: edgion.io/gateway-controller
```

## Examples

### Example 1: Basic Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

### Example 2: Configuration with Parameters

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion-custom
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: custom-config
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: custom-config
spec:
  server:
    threads: 4
    gracePeriodSeconds: 30
```

## EdgionGatewayConfig Reference

`EdgionGatewayConfig` is an Edgion extension CRD, referenced through the GatewayClass `parametersRef`, providing gateway-level advanced configuration.

### spec.server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| threads | integer | CPU cores | Worker thread count |
| workStealing | boolean | true | Enable work-stealing scheduling |
| gracePeriodSeconds | integer | 30 | Graceful shutdown grace period (seconds) |
| gracefulShutdownTimeoutS | integer | 10 | Graceful shutdown timeout (seconds) |
| upstreamKeepalivePoolSize | integer | 128 | Upstream keepalive connection pool size |
| enableCompression | boolean | false | Enable downstream response compression |
| downstreamKeepaliveRequestLimit | integer | 1000 | Max requests per downstream connection |

#### downstreamKeepaliveRequestLimit

Limits the maximum number of HTTP requests a single downstream TCP connection can serve. The connection is closed after reaching the limit. Equivalent to Nginx's [`keepalive_requests`](https://nginx.org/en/docs/http/ngx_http_core_module.html#keepalive_requests).

- **Per-connection**: Each TCP connection has an independent counter, not a global limit
- **HTTP/1.1 only**: HTTP/2 multiplexing is not affected by this limit
- **Default 1000**: Consistent with Nginx. Set to `0` to disable the limit

**Purpose**: Prevents memory accumulation and load imbalance caused by long-lived single connections.

### spec.httpTimeout

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| client.readTimeout | duration | 60s | Client read timeout |
| client.writeTimeout | duration | 60s | Client write timeout |
| client.keepaliveTimeout | duration | 75s | HTTP keepalive timeout |
| backend.defaultConnectTimeout | duration | 5s | Backend connection timeout |
| backend.defaultRequestTimeout | duration | 60s | Backend request timeout |
| backend.defaultIdleTimeout | duration | 300s | Backend connection pool idle timeout |
| backend.defaultMaxRetries | integer | 3 | Maximum retry count |

### Full Example

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: production-config
spec:
  server:
    threads: 4
    workStealing: true
    gracePeriodSeconds: 30
    upstreamKeepalivePoolSize: 256
    downstreamKeepaliveRequestLimit: 1000
  httpTimeout:
    client:
      readTimeout: 60s
      writeTimeout: 60s
      keepaliveTimeout: 75s
    backend:
      defaultConnectTimeout: 5s
      defaultRequestTimeout: 60s
      defaultIdleTimeout: 300s
```

## Related Documentation

- [Gateway Overview](./overview.md)
