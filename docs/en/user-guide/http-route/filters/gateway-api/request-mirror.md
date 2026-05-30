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

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `backendRef` | Object | **Required** | The backend to mirror traffic to: `{name, port}` |
| `fraction` | Object | mirror all | Portion of requests to mirror: `{numerator, denominator}` |
| `connectTimeoutMs` | Integer | `1000` | Connect timeout for the mirror request, in milliseconds |
| `writeTimeoutMs` | Integer | `1000` | Write timeout for the mirror request, in milliseconds |
| `maxBufferedChunks` | Integer | `5` | Maximum number of body chunks buffered for mirroring |
| `mirrorLog` | Boolean | `true` | Whether the mirror produces its own access log entry |
| `maxConcurrent` | Integer | `1024` | Maximum number of concurrent in-flight mirror tasks per gateway process |
| `channelFullTimeoutMs` | Integer | `0` | How long to wait for queue space when the mirror channel is full, in milliseconds; `0` abandons the mirror immediately |

### `fraction` sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `fraction.numerator` | Integer | none | Numerator of the mirrored fraction |
| `fraction.denominator` | Integer | `100` | Denominator of the mirrored fraction |

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
            connectTimeoutMs: 1000
            writeTimeoutMs: 1000
            maxBufferedChunks: 5
            maxConcurrent: 1024
      backendRefs:
        - name: api-service
          port: 8080
```

## Notes

- The mirror is non-blocking: the client always receives the primary backend's response, and the mirrored response is discarded.
- If the mirror queue is full and `channelFullTimeoutMs` elapses, the mirror for that request is dropped rather than delaying the original request.
- This is the standard Gateway API mirror, which targets a `backendRef`. For richer mirroring options — such as mirroring to an external destination or finer control beyond a `backendRef` — see the [Edgion RequestMirror plugin](../edgion-plugins/request-mirror.md).

## Related documents

- [Edgion RequestMirror plugin](../edgion-plugins/request-mirror.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
