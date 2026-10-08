# GraphQL operation routing

`GraphqlOperationRouter` sends a complete GraphQL request to one declared HTTPRoute
Service backend using its selected `(operationType, operationName)` pair. It does
not split fields, execute GraphQL, or translate GraphQL into SQL or Elasticsearch
queries. Backends must accept GraphQL; database services need a GraphQL facade.

Attach the plugin to an HTTPRoute rule using ExtensionRef. Put authentication,
request rewrites, and optional `GraphqlRateLimit` before the router. Place the
router last and do not mutate its admitted document or URI afterward.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: graphql-router
  namespace: apps
spec:
  requestPlugins:
    - type: GraphqlOperationRouter
      config:
        rules:
          - operationType: query
            operationName: GetUser
            backendRef:
              name: users
          - operationType: mutation
            operationName: CreateOrder
            backendRef:
              name: orders
        defaultBackendRef:
          name: core
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: graphql
  namespace: apps
spec:
  parentRefs:
    - name: api-gateway
  rules:
    - matches:
        - path:
            type: Exact
            value: /graphql
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: graphql-router
      backendRefs:
        - name: users
          port: 8080
        - name: orders
          port: 8080
        - name: core
          port: 8080
```

Every mapping and the optional default must reference a Service backend declared
in the same attaching rule, including mappings unused by a particular request.
Other rules and routes do not qualify. Omitted namespace means the HTTPRoute
namespace. Namespace/name keys must be unique even if their ports differ.
Cross-namespace references still require ReferenceGrant. Removing any declared
target invalidates the attachment until its mappings and backendRefs agree again.
Bypass and conditions cannot legalize an undeclared target; source-disabled entries
are inert. Controller withdraws invalid rules and preserves valid siblings; a
withdrawn rule can produce 404, while Gateway seals a matched invalid attachment
with 500. Plugin updates and route updates both trigger revalidation.

POST requires an `application/json` object with `query`, optional `operationName`,
and optional object `variables`. GET accepts those parameters in the URL and
requires no body. GET mutations return 405. Names are case-sensitive; when a
document contains multiple operations, `operationName` must select exactly one.
An anonymous or unmapped operation uses only the explicit default; without one it
returns 400. A failed selected target never falls back to another backend.
The complete original POST body or GET URL is forwarded unchanged by this plugin.

The envelope limit is 64 KiB; a smaller entry body limit is supported. Parsing has
independent JSON, token, recursion and work bounds and shares four nonwaiting
process-wide evaluation permits with GraphqlRateLimit. Busy admission returns 503.
Batching, subscriptions, persisted queries, nonempty extensions, multipart,
compressed requests, delegated HTTPRoutes, global/Gateway/backendRef attachments,
and AI or gRPC transcoding owners are unsupported. Another active backend-selection
publisher in the effective rule chain makes the attachment invalid. Explicitly
bypassing a conflicting publisher can restore a structurally valid chain.

The router validates operation selection and bounded syntax, not backend SDL or
field authorization. GraphqlRateLimit remains optional and keeps its own static
SDL and Redis quota configuration. Its debit is not refunded if later routing or
upstream processing fails.

Local HTTP rejections use the [GraphQL JSON error contract](graphql-errors.md).
