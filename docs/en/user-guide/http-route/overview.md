# HTTPRoute Overview

HTTPRoute is the core resource in Gateway API for defining HTTP routing rules.

## Resource Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: default
spec:
  parentRefs:           # Which Gateway to bind to
    - name: my-gateway
      sectionName: http
  hostnames:            # Hostnames to match
    - "example.com"
  rules:                # List of routing rules
    - matches:          # Match conditions
        - path:
            type: PathPrefix
            value: /api
      filters:          # Filters (optional)
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-Custom-Header
                value: "value"
      backendRefs:      # Backend services
        - name: api-service
          port: 8080
          weight: 100
```

## Core Concepts

### parentRefs - Parent Resource References

Specifies which listener of which Gateway this route binds to:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | string | Yes | Gateway name |
| namespace | string | | Gateway namespace (defaults to the route's namespace) |
| sectionName | string | | Listener name |

### hostnames - Hostnames

Matches the Host header of incoming requests:

- Exact match: `example.com`
- Wildcard match: `*.example.com`

### rules - Routing Rules

Each rule consists of:
- **matches**: Match conditions (path, headers, query parameters, method)
- **filters**: Request/response processing
- **backendRefs**: List of backend services

## Related Documentation

- [Match Rules](./matches/README.md)
- [Filters](./filters/README.md)
- [Backend Configuration](./backends/README.md)
- [Resilience Configuration](./resilience/README.md)
