# Load Balancing Algorithm Configuration Guide

> **🔌 Edgion Extension**
> 
> Configuring load balancing algorithms via `ExtensionRef` is an Edgion extension feature.

This document explains how to configure optional load balancing algorithms through the HTTPRoute `extensionRef`.

## Overview

Edgion uses the RoundRobin load balancing algorithm by default. You can enable additional algorithms for specific services through the following options:
- **Ketama**: Consistent hashing algorithm
- **FnvHash**: FNV hashing algorithm
- **LeastConnection**: Least connections algorithm

## Configuration Method

Specify the load balancing algorithm directly in the HTTPRoute filter via `extensionRef.name`. The algorithm configuration is automatically applied to all backendRefs in that rule.

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
            name: ketama  # Single algorithm
      backendRefs:
        - name: my-service
          port: 8080
```

## Supported Algorithms

| Algorithm Name | Aliases | Description |
|---------------|---------|-------------|
| `ketama` | `consistent-hash` | Consistent hashing, suitable for caching scenarios |
| `fnvhash` | `fnv-hash` | FNV hashing algorithm |
| `leastconn` | `least-connection`, `leastconnection`, `least_connection` | Least connections algorithm |

## How It Works

1. **Policy Extraction**: When an HTTPRoute is created or updated, Edgion scans all `ExtensionRef` type filters
2. **Algorithm Parsing**: Parses the algorithm name from `extensionRef.name`
3. **Service Mapping**: Applies the algorithm to all services specified by backendRefs in that rule
4. **Policy Storage**: Stores the service-to-algorithm mapping in the global PolicyStore
5. **On-Demand Loading**: When an EndpointSlice is created, the corresponding load balancer is initialized on demand based on the service's policy

## Lifecycle Management

- **Reference Counting**: PolicyStore tracks how many HTTPRoutes reference each service
- **Automatic Cleanup**: When the last HTTPRoute referencing a service is deleted, the corresponding policy is automatically cleaned up
- **Update Handling**: When an HTTPRoute is updated, the related policy configuration is automatically refreshed

### Manual Policy Deletion

In addition to automatic cleanup, you can also manually delete load balancing policies for a specific HTTPRoute:

```rust
use edgion::core::lb::optional_lb::get_global_policy_store;

// Get the global policy store
let store = get_global_policy_store();

// Delete policies by resource key
store.delete_lb_policies_by_resource_key("default/my-route");
```

**Notes:**
- This operation deletes all policy references from the specified HTTPRoute to all services
- If a service is only referenced by this HTTPRoute, its policy will be completely removed
- If a service is also referenced by other HTTPRoutes, its policy is retained

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
            name: ketama  # Consistent hashing
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
            name: leastconn  # Least connections
      backendRefs:
        - name: user-api
          port: 8080
    
    - matches:
        - path:
            type: PathPrefix
            value: /orders
      filters:
        - type: ExtensionRef
          extensionRef:
            name: leastconn
      backendRefs:
        - name: order-api
          port: 8080
```

## Notes

1. **Algorithm format**: Algorithm names in `extensionRef.name` are case-insensitive
2. **Single configuration**: Each rule can only have one load balancing algorithm configured
3. **Scope**: The algorithm configuration applies to all backendRefs within the same rule
4. **Default behavior**: Services without a configured policy continue to use the default RoundRobin algorithm

## Troubleshooting

View related information in the logs:

```bash
# View policy extraction logs
kubectl logs <edgion-pod> | grep "LB policy"

# View policy application logs
kubectl logs <edgion-pod> | grep "Added LB policies"

# View policy cleanup logs
kubectl logs <edgion-pod> | grep "Removed LB policies"
```

Common issues:

- **Policy not taking effect**: Check that the algorithm name in `extensionRef.name` is correct
- **Incorrect algorithm name**: Use supported algorithm names or aliases (see the algorithm table above)
