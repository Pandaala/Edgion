# GRPCRoute Matching Rules

GRPCRoute matches inbound gRPC calls by service and method name, and optionally by gRPC metadata (headers). A rule fires when **all** conditions inside one `matches` entry are satisfied; listing multiple `matches` entries is an OR.

## Match types

| Match type | Description |
|------------|-------------|
| Method (`Exact`) | Exact match on the gRPC `service` and/or `method` name |
| Method (`RegularExpression`) | Regular-expression match on `service` and/or `method` |
| Headers (`Exact`) | Exact match on a gRPC metadata key/value (request header) |
| Headers (`RegularExpression`) | Regular-expression match on a metadata value |

A gRPC request path has the form `/<package>.<Service>/<Method>`. In a method match, `service` is the fully qualified `<package>.<Service>` and `method` is the RPC name.

## Method matching

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: user-service-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
      backendRefs:
        - name: user-service
          port: 9000
```

Omitting `method` matches any RPC on the listed `service`; omitting `service` matches the named `method` on any service.

## Combining conditions (AND within one match)

A single `matches` entry combines its `method` and `headers` conditions with AND — the call must satisfy every condition:

```yaml
spec:
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
          headers:
            - type: Exact
              name: x-api-version
              value: v2
      backendRefs:
        - name: user-service-v2
          port: 9000
```

## Multiple matches (OR across entries)

List several `matches` entries to route a set of RPCs to the same backend; a call matching **any** entry is accepted:

```yaml
spec:
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
        - method:
            type: Exact
            service: user.UserService
            method: ListUsers
      backendRefs:
        - name: user-service
          port: 9000
```

## Regular-expression matching

Route every RPC on a service with a method-name pattern:

```yaml
spec:
  rules:
    - matches:
        - method:
            type: RegularExpression
            service: user.UserService
            method: "Get.*"
      backendRefs:
        - name: user-readonly-service
          port: 9000
```

## Precedence

When a call matches more than one rule, the most specific rule wins: an `Exact` method match is preferred over a `RegularExpression` match, and a match with more header conditions is preferred over one with fewer. Define narrower rules ahead of broader catch-all rules.

## Related documents

- [GRPCRoute Overview](../overview.md)
- [GRPCRoute Filters](../filters/overview.md)
- [GRPCRoute Backends](../backends/overview.md)
