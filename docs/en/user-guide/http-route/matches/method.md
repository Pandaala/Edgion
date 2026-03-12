# HTTP Method Matching

Route matching based on HTTP request methods.

## Configuration

```yaml
matches:
  - method: GET
```

Supported methods:
- `GET`
- `POST`
- `PUT`
- `DELETE`
- `PATCH`
- `HEAD`
- `OPTIONS`

## Examples

### Example 1: RESTful Routing

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: restful-routing
spec:
  parentRefs:
    - name: my-gateway
  rules:
    # GET /users -> query service
    - matches:
        - path:
            type: PathPrefix
            value: /users
          method: GET
      backendRefs:
        - name: user-query-service
          port: 8080
    # POST/PUT/DELETE /users -> write service
    - matches:
        - path:
            type: PathPrefix
            value: /users
          method: POST
        - path:
            type: PathPrefix
            value: /users
          method: PUT
        - path:
            type: PathPrefix
            value: /users
          method: DELETE
      backendRefs:
        - name: user-command-service
          port: 8080
```

### Example 2: Read-Only Gateway

```yaml
rules:
  - matches:
      - method: GET
      - method: HEAD
      - method: OPTIONS
    backendRefs:
      - name: backend
        port: 8080
```

Only allows read-only requests to pass through.

## Related Documentation

- [Path Matching](./path.md)
- [Header Matching](./headers.md)
