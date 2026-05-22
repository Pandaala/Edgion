---
name: backends-features-detail
description: Full reference for backend service discovery, health check configuration, and load balancing strategies.
---

# Backend Features

## Service Discovery

Edgion discovers backend endpoints via the Kubernetes EndpointSlice API. The Controller continuously syncs EndpointSlice resources via K8s watch, and pushes endpoint information to the Gateway via gRPC ConfigSync.

### Service Resource

Routes reference Services via `backendRefs`:

```yaml
backendRefs:
  - name: my-service          # Service name
    port: 8080                # Service port
    weight: 100               # Traffic weight
```

Service changes (e.g., port updates) automatically trigger reprocessing of dependent routes via ServiceRefManager.

## Health Checks

Configure health checks via Service annotations:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-service
  annotations:
    edgion.io/health-check: |
      type: http
      path: /healthz
      interval: 10s
      timeout: 5s
      unhealthyThreshold: 3
      healthyThreshold: 2
      port: 8080
```

### Health Check Schema

```yaml
edgion.io/health-check: |
  type: String              # http | tcp | grpc
  # HTTP mode
  path: String              # Health check path (required for http)
  expectedStatuses: [u16]   # Expected HTTP status codes (http only)
  host: String              # Host header override (http only)
  # gRPC mode
  grpcServiceName: String   # Service name for gRPC Health Check (grpc only; default empty = check the entire server)
  # Common
  interval: Duration        # Check interval
  timeout: Duration         # Timeout
  unhealthyThreshold: u32   # Unhealthy threshold
  healthyThreshold: u32     # Healthy recovery threshold
  port: u16                 # Check port (optional, defaults to the Service port)
```

## Circuit Breaker

Configure passive circuit breaking (per-endpoint ejection) via Service annotations:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-service
  annotations:
    edgion.io/circuit-breaker: |
      consecutiveErrors: 5
      ejectionSeconds: 30
      maxEjectionPercent: 50
```

### Circuit Breaker Schema

```yaml
edgion.io/circuit-breaker: |
  consecutiveErrors: u32    # Eject after this many consecutive failures (default 5, minimum 1)
  ejectionSeconds: u64      # Cooldown duration after ejection in seconds (default 30, minimum 1)
  maxEjectionPercent: u32   # Maximum ejection ratio percent (default 50, 0 = unlimited)
```

The configuration is attached to the Service, while ejection tracking works independently inside the EndpointSlice — complementary to health checks.

## Load Balancing Strategies

Edgion supports 5 load balancing algorithms:

| Algorithm | Description | Use Case |
|-----------|-------------|----------|
| `RoundRobin` | Round-robin (default) | General use |
| `EWMA` | Exponentially Weighted Moving Average (based on response time) | Uneven backend performance |
| `LeastConn` | Least connections | Long-connection scenarios |
| `ConsistentHash` | Consistent hashing | When session affinity is required |
| `Weighted` | Weighted round-robin | Distribution by weight |

The load balancing strategy is configured via the weight field in backendRefs and via EdgionGatewayConfig.

## BackendTLSPolicy

For gateway-to-backend TLS configuration, see [12-backend-tls-policy.md](12-backend-tls-policy.md).
