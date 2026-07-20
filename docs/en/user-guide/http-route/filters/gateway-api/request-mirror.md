# RequestMirror

## Overview

`RequestMirror` copies a fraction of incoming requests to a second backend while the original request is still served normally by the route's primary backend. It is a standard Gateway API filter and runs in the request stage. Mirroring is fire-and-forget: the mirrored response is discarded and never affects the client, so it is safe for shadow testing a new service version against live traffic.

## Configuration

```yaml
filters:
  - type: RequestMirror
    requestMirror:
      backendRef:
        name: mirror-service
        port: 8080
      fraction:
        numerator: 10
        denominator: 100
```

The filter accepts only the standard Gateway API fields:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `backendRef` | Object | **Required** | The backend to mirror traffic to: `{name, port}` |
| `fraction` | Object | mirror all | Portion of requests to mirror: `{numerator, denominator}` |
| `percent` | Integer | none | Percentage of requests to mirror (0-100); alternative to `fraction` |

### `fraction` sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `fraction.numerator` | Integer | none | Numerator of the mirrored fraction |
| `fraction.denominator` | Integer | `100` | Denominator of the mirrored fraction |

### Tuning annotations

Mirror tuning knobs are set as `edgion.io/mirror-*` annotations on the
HTTPRoute's `metadata.annotations`, not as filter fields. The Gateway API CRD
is a structural schema, so any non-standard field inside `requestMirror`
would be silently pruned by the Kubernetes API server; annotations survive.
Each annotation applies to every `RequestMirror` filter under the route.

| Annotation | Type | Default | Description |
|------------|------|---------|-------------|
| `edgion.io/mirror-connect-timeout-ms` | Integer | `1000` | Connect timeout for the mirror request, in milliseconds |
| `edgion.io/mirror-write-timeout-ms` | Integer | `1000` | Write timeout for the mirror request, in milliseconds |
| `edgion.io/mirror-max-buffered-chunks` | Integer | `5` | Maximum number of body chunks buffered for mirroring |
| `edgion.io/mirror-log` | Boolean | `true` | Whether the mirror produces its own access log entry |
| `edgion.io/mirror-max-concurrent` | Integer | `1024` | Maximum number of concurrent in-flight mirror tasks per gateway process |
| `edgion.io/mirror-channel-full-timeout-ms` | Integer | `0` | How long to wait for queue space when the mirror channel is full, in milliseconds; `0` abandons the mirror immediately (values above `1000` are clamped) |

An invalid annotation value is logged and the field falls back to its default.

## Examples

### Example 1: Mirror all traffic to a shadow service

Send a copy of every request to a new version of the service for comparison.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: shadow-route
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
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: api-service-v2
              port: 8080
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Mirror 10% with bounded resource usage

Mirror only a tenth of traffic and cap the concurrency and buffering so the mirror cannot back-pressure the primary path.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: sampled-shadow-route
  namespace: production
  annotations:
    edgion.io/mirror-connect-timeout-ms: "1000"
    edgion.io/mirror-write-timeout-ms: "1000"
    edgion.io/mirror-max-buffered-chunks: "5"
    edgion.io/mirror-max-concurrent: "1024"
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: api-service-v2
              port: 8080
            # Mirror 10% of requests
            fraction:
              numerator: 10
              denominator: 100
      backendRefs:
        - name: api-service
          port: 8080
```

## Notes

- The mirror is non-blocking: the client always receives the primary backend's response, and the mirrored response is discarded.
- If the mirror queue is full and `edgion.io/mirror-channel-full-timeout-ms` elapses, the mirror for that request is dropped rather than delaying the original request.
- This is the standard Gateway API mirror, which targets a `backendRef`. For richer mirroring options — such as inline tuning fields or finer control beyond a `backendRef` — see the [Edgion RequestMirror plugin](../edgion-plugins/request-mirror.md), whose config embeds the tuning knobs directly (its CRD is not subject to structural-schema pruning).

## Related documents

- [Edgion RequestMirror plugin](../edgion-plugins/request-mirror.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
