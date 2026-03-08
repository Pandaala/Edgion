# Backend Active Health Check

> **🔌 Edgion Extension**
>
> This feature enables active probing for backends via the `edgion.io/health-check` Annotation, and is an Edgion extension capability.

## Overview

Active health checks periodically probe backend instances (HTTP/TCP) from the gateway side, and automatically skip unhealthy instances during load balancing selection.
This feature is primarily used when backends are reachable but the application layer is abnormal, or as a supplementary readiness check in non-Kubernetes scenarios.

## Quick Start

The most common usage is to configure it on a `Service`:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: default
  annotations:
    edgion.io/health-check: |
      active:
        type: http
        path: /healthz
        interval: 10s
        timeout: 3s
        healthyThreshold: 2
        unhealthyThreshold: 3
        expectedStatuses:
          - 200
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

## Annotation Reference

| Annotation | Applicable Resources | Type | Default | Description |
|------------|---------------------|------|---------|-------------|
| `edgion.io/health-check` | `Service` / `EndpointSlice` / `Endpoints` | YAML string | None | Configure active health check parameters |

Example value:

```yaml
active:
  type: tcp
  port: 6379
  interval: 5s
  timeout: 1s
  healthyThreshold: 2
  unhealthyThreshold: 3
```

## Configuration Parameters

YAML structure of `edgion.io/health-check`:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `active` | object | No | None | Active probe configuration; health check is disabled if not configured |

`active` sub-fields:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `type` | `http` \| `tcp` | No | `http` | Probe type |
| `path` | string | No | `/` | HTTP probe path (only effective for `http`) |
| `port` | uint16 | No | Uses backend port | Probe port override |
| `interval` | duration string | No | `10s` | Probe interval |
| `timeout` | duration string | No | `3s` | Single probe timeout |
| `healthyThreshold` | uint32 | No | `2` | Number of consecutive successes to recover healthy status |
| `unhealthyThreshold` | uint32 | No | `3` | Number of consecutive failures to mark unhealthy |
| `expectedStatuses` | `[]uint16` | No | `[200]` | Expected HTTP status codes (only effective for `http`) |
| `host` | string | No | None | HTTP Host header override (only effective for `http`) |

## Behavior Details and Priority

### Resource-Level Priority

The configuration priority for the same `service_key` is:

1. `EndpointSlice` annotation
2. `Endpoints` annotation
3. `Service` annotation

### EndpointSlice Conflict Handling

When multiple `EndpointSlice` resources under the same service have `edgion.io/health-check` set with inconsistent configurations:

- `EndpointSlice`-level configuration will be disabled (to avoid non-deterministic behavior)
- Falls back to the next level (`Endpoints` or `Service`)

### Runtime Minimum Values (Safety Floor)

Even if smaller values are configured, runtime will enforce safety floors:

- `interval` is executed at a minimum of `1s`
- `timeout` is executed at a minimum of `100ms`

### How Health Status Takes Effect

Edgion does not directly remove backends from the LB pool. Instead, it filters out unhealthy instances during the `select_with()` selection phase.
Therefore:

- Health status changes take effect immediately on routing
- The backend list is still maintained by the `EndpointSlice/Endpoints` data source

## Scenario Examples

### Scenario 1: Standard K8s HTTP Service Health Check (Recommended)

Configure HTTP probing on the `Service` to cover all backends of that service.

```yaml
metadata:
  annotations:
    edgion.io/health-check: |
      active:
        type: http
        path: /healthz
        interval: 10s
        timeout: 3s
        healthyThreshold: 2
        unhealthyThreshold: 3
        expectedStatuses: [200, 204]
```

### Scenario 2: Non-K8s / Endpoint Mode (Endpoints Only)

```yaml
apiVersion: v1
kind: Endpoints
metadata:
  name: legacy-backend
  namespace: default
  annotations:
    edgion.io/health-check: |
      active:
        type: tcp
        port: 9000
        interval: 5s
        timeout: 1s
```

### Scenario 3: EndpointSlice-Level Overrides Service-Level

```yaml
# Service-level default configuration
metadata:
  annotations:
    edgion.io/health-check: |
      active:
        type: http
        path: /healthz

---
# A specific EndpointSlice overrides with TCP probing
metadata:
  annotations:
    edgion.io/health-check: |
      active:
        type: tcp
        port: 8081
```

## Notes

1. `expectedStatuses` only applies to `http` mode; `tcp` mode ignores this field.
2. `path` must start with `/`, otherwise the configuration is considered invalid and ignored.
3. `healthyThreshold` and `unhealthyThreshold` must be greater than or equal to `1`.
4. Services without health check configuration are not probed and participate in load balancing as "healthy" by default.

## Current Limitations

1. **Active probing only**
   - What: Passive health checks (automatic degradation based on request failures) are not yet implemented
   - Workaround: Use active probing to shorten failure detection time
   - Tracking: Planned

2. **HTTP probing supports plaintext HTTP only**
   - What: There is currently no dedicated HTTPS probe configuration option
   - Workaround: Use TCP probing or expose a plaintext HTTP health endpoint on the internal network
   - Tracking: Planned

## Troubleshooting

### Issue 1: Annotation is configured but does not seem to take effect

Cause: The annotation YAML parsing failed or field validation failed, causing it to be ignored.
Solution: Check YAML indentation, `path`, threshold values, and duration format.

### Issue 2: Probing too frequently causes increased backend pressure

Cause: `interval` is configured too low.
Solution: Increase `interval`; for production environments, start from `5s~30s`.

### Issue 3: Unstable behavior with multiple EndpointSlices for the same service

Cause: Configuration conflicts trigger EndpointSlice-level disabling and fallback.
Solution: Unify health check configuration across EndpointSlices, or only keep the Service-level configuration.

## Related Documentation

- [Service Reference](./service-ref.md)
- [Weight Configuration](./weight.md)
- [Backend TLS](./backend-tls.md)
- [Timeout Configuration](../resilience/timeouts.md)
