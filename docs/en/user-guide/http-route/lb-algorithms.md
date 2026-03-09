# Load Balancing Algorithm Configuration Guide

> **🔌 Edgion Extension**
> 
> Configuring load balancing algorithms via `ExtensionRef` is an Edgion extension feature.

This document explains how to configure load balancing algorithms through the HTTPRoute `extensionRef`.

## Overview

Edgion uses **RoundRobin (weighted round-robin)** as the default load balancing algorithm. You can enable other algorithms for specific services:

- **ConsistentHash (Ketama)**: Consistent hashing, suitable for caching scenarios
- **LeastConnection**: Least connections, suitable for long-lived connection scenarios
- **EWMA**: Latency-aware algorithm based on exponential weighted moving average

## Supported Algorithms

| Algorithm Name | Aliases | Description | Use Case |
|---------------|---------|-------------|----------|
| `ketama` | `consistent-hash`, `consistent` | Consistent hashing (Ketama), routes same key to same backend | Caching, session affinity |
| `leastconn` | `least-connection`, `leastconnection`, `least_connection` | Selects the backend with fewest active connections | gRPC streaming, WebSocket, long connections |
| `ewma` | - | Selects the backend with lowest EWMA latency | Heterogeneous backends, mixed hardware |

When not configured, the default is **RoundRobin** with weight support.

## Configuration

Specify the load balancing algorithm in the HTTPRoute filter via `extensionRef.name`. The algorithm configuration is automatically applied to all backendRefs in that rule.

### Basic Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - api.example.com
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            name: ketama
      backendRefs:
        - name: my-service
          port: 8080
```

## Algorithm Details

### RoundRobin (Default)

- Weighted round-robin: higher `weight` means higher selection probability
- Single atomic counter increment, lock-free selection
- Supports backend health filtering and fallback

### ConsistentHash

- Ketama-based consistent hash ring
- Same hash key always maps to the same backend when backend list is unchanged
- When backends change, only ~1/N of keys remap
- Hash key extracted from Header / Cookie / Query / Source IP
- Falls back to RoundRobin when hash key cannot be extracted

**ConsistentHash hashOn configuration**:

```yaml
extensionRef:
  name: ketama:header:X-User-Id    # Hash by header
  # or: ketama:cookie:session_id   # Hash by cookie
  # or: ketama:query:user_id       # Hash by query parameter
  # or: ketama:source_ip           # Hash by source IP
  # or: ketama                     # Default: hash by source IP
```

### LeastConnection

- Selects the backend with the fewest active connections
- Service-scoped isolation: same IP under different services counts independently
- Increments on request start, decrements on completion
- New backends are preferred (connection count = 0)
- Removed backends drain gracefully: no new requests, existing ones complete normally

### EWMA

- Selects the backend with the lowest EWMA latency
- Formula: `new = alpha × latency + (1 - alpha) × old`, default alpha = 10%
- Latency updated after each request completes
- New backends default to 1ms latency, briefly preferred, then converge to actual latency
- Service-scoped isolation

## How It Works

1. **Policy Extraction**: When an HTTPRoute is created or updated, Edgion scans all `ExtensionRef` type filters
2. **Algorithm Parsing**: Parses the algorithm name from `extensionRef.name`
3. **Service Mapping**: Applies the algorithm to all services specified by backendRefs in that rule
4. **Policy Storage**: Stores the service-to-algorithm mapping in the global PolicyStore
5. **On-Demand Loading**: When a request arrives, the corresponding LB algorithm is selected based on the service's policy

## Lifecycle Management

- **Reference Counting**: PolicyStore tracks how many HTTPRoutes reference each service
- **Automatic Cleanup**: When the last HTTPRoute referencing a service is deleted, the corresponding policy is automatically cleaned up
- **Cache Management**: When the backend list changes, the LB cache (RR selector, CH hash ring) is automatically cleared and rebuilt on next request
- **Runtime State Cleanup**: When a Service is deleted, all runtime state (connection counts, EWMA values) is automatically cleaned up

## Example Scenarios

### Scenario 1: Cache Service with Consistent Hashing

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cache-route
  namespace: default
spec:
  parentRefs:
    - name: gateway
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            name: ketama:header:X-Cache-Key
      backendRefs:
        - name: redis-cache
          port: 6379
```

### Scenario 2: API Service with Least Connections

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-routes
  namespace: prod
spec:
  parentRefs:
    - name: api-gateway
  hostnames:
    - api.mycompany.com
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /users
      filters:
        - type: ExtensionRef
          extensionRef:
            name: leastconn
      backendRefs:
        - name: user-api
          port: 8080
```

### Scenario 3: gRPC Service with EWMA

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: grpc-route
  namespace: prod
spec:
  parentRefs:
    - name: api-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /grpc
      filters:
        - type: ExtensionRef
          extensionRef:
            name: ewma
      backendRefs:
        - name: grpc-service
          port: 50051
```

## Notes

1. **Algorithm format**: Algorithm names in `extensionRef.name` are case-insensitive
2. **Single configuration**: Each rule can only have one load balancing algorithm configured
3. **Scope**: The algorithm configuration applies to all backendRefs within the same rule
4. **Default behavior**: Services without a configured policy use the default RoundRobin algorithm
5. **Backend weight**: All algorithms support `weight` configuration
6. **Health check integration**: All algorithms automatically integrate health check filtering

## Troubleshooting

View related information in the logs:

```bash
# View policy extraction logs
kubectl logs <edgion-pod> | grep "LB policy"

# View policy application logs
kubectl logs <edgion-pod> | grep "Added LB policies"

# View policy cleanup logs
kubectl logs <edgion-pod> | grep "Removed LB policies"

# View backend draining logs
kubectl logs <edgion-pod> | grep "Backend marked as draining"

# View service runtime state cleanup logs
kubectl logs <edgion-pod> | grep "Removed service runtime state"
```

Common issues:

- **Policy not taking effect**: Check that the algorithm name in `extensionRef.name` is correct
- **Incorrect algorithm name**: Use supported algorithm names or aliases (see the algorithm table above)
- **ConsistentHash instability**: Check if the hash key is empty (falls back to RR when empty)
