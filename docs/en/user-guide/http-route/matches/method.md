# HTTP Method Matching

Method matching routes requests by their HTTP method.

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
- `CONNECT`

## Configuration reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `method` | Enum | none | HTTP method to match. Use uppercase method tokens |

## How method conditions combine

- A single match block holds at most one `method`. It is ANDed with `path`, `headers`, and `queryParams` in the same block.
- To match several methods, list them as separate match blocks; multiple matches in a rule are ORed.

## Examples

### Example 1: Split reads and writes

`GET /users` goes to the query service; `POST`, `PUT`, and `DELETE` to the command service. Each method is its own ORed match block:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: restful-routing
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
    # POST/PUT/DELETE /users -> command service
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

### Example 2: Read-only gateway

Only read-only methods pass through (three ORed match blocks):

```yaml
rules:
  - matches:
      - method: GET
      - method: HEAD
      - method: OPTIONS
    backendRefs:
      - name: backend-service
        port: 8080
```

## Related documents

- [Path matching](./path.md)
- [Header matching](./headers.md)
- [Query parameter matching](./query-params.md)
