# GRPCRoute Matching Rules

GRPCRoute matches inbound gRPC calls by service and method name, and optionally by gRPC metadata (headers). A rule fires when **all** conditions inside one `matches` entry are satisfied; listing multiple `matches` entries is an OR.

## Hostnames

`spec.hostnames` accepts up to 16 lowercase DNS names, including short names such as `grpc`. `*.example.com` matches `api.example.com` and `a.b.example.com`, but not `example.com`. IP literals, malformed names and a bare `*` are invalid; one invalid entry rejects the whole list. An omitted or empty list leaves the Route hostname unrestricted, while Listener checks still apply.

Exact declared names precede matching wildcard suffixes, longest suffix first, then hostless routes. Listener intersection narrows eligibility without increasing Route hostname priority. Unmatched method or header predicates may continue to other eligible candidates, but cannot bypass a more-specific Listener. Certificate selection and certificate wildcard verification use separate, unchanged rules.

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

For example, `method: {method: GetUser}` matches `/user.UserService/GetUser`
and `/admin.AdminService/GetUser`. Omitted `type` defaults to `Exact`.
Omitting the outer method matcher leaves service/method unconstrained, so a
header-only match remains valid. Explicit `method: {}` and empty Exact fields
are invalid. For regex, an absent/empty field is unconstrained only when the
other field is non-empty. Invalid method names or patterns invalidate the whole
rule, not valid sibling rules; Controller status reports `PartiallyInvalid`, or
`Accepted=False / UnsupportedValue` when all effective rules are invalid.

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

Matching candidates are ordered by:

1. Most specific matching hostname: exact, then longest wildcard suffix, then no hostname constraint.
2. Most characters in the configured service, then method (pattern lengths for regex).
3. Most header conditions.
4. Oldest route creation time, then alphabetical `namespace/name`.
5. First matching rule, then match entry within that rule.

`Exact` is not an extra priority tier ahead of regex. A candidate must satisfy
all its conditions; a header or listener mismatch continues to the next
candidate, including broader hostnames. A longer hostname that does not match
the request cannot increase a route's priority.

## Related documents

- [GRPCRoute Overview](../overview.md)
- [GRPCRoute Filters](../filters/overview.md)
- [GRPCRoute Backends](../backends/overview.md)
