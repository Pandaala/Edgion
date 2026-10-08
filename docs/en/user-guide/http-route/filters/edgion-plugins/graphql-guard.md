# GraphqlGuard

GraphqlGuard protects a GraphQL HTTP endpoint without requiring Redis or a schema.
Attach it in requestPlugins, after authentication and request rewrites and before
upstream forwarding. Later plugins must not change the admitted operation.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: graphql-protection
spec:
  requestPlugins:
    - type: GraphqlGuard
      config:
        maxDepth: 20
        maxAliases: 64
        maxRootFields: 32
        maxFields: 1024
        allowIntrospection: false
```

Field depth starts at one. Counts cover the selected operation, including reachable
fragments, repeated occurrences and every skip/include branch. Inline and named
fragments do not add field depth. Aliases do not hide introspection: __schema and
__type are denied by default; __typename is allowed and counted.

Omitted limits use the values above. Explicit null disables that business limit;
the envelope, parser, recursion, work and shared admission limits remain active.
Count limits accept 0..4096; zero forbids that shape. maxDepth accepts 1..32.
The plugin returns 400 for shape or cost violations and preserves original request
bytes. It does not implement backend field authorization or full GraphQL execution.

For an optional cost ceiling, configure maxCost and static schema, with optional
costRules using the [existing cost semantics](graphql-rate-limit.md). No Redis
reference or policies are required. The total includes the cost model's request
base charge. For example:

```yaml
type: GraphqlGuard
config:
  maxCost: 100
  schema: |
    type Query { users(first: Int = 10): [User] }
    type User { id: ID }
  costRules:
    - field: Query.users
      multipliers: [first]
```

schema and nonempty costRules require maxCost. allowIntrospection must be false
when maxCost is selected. A null maxDepth disables shape depth checking but costing
retains its independent supported depth ceiling of 32. Optional costing also
validates the document against the supported SDL subset.

Use ordinary strict GraphQL GET or JSON POST requests under the 64KiB envelope cap.
The shared frontend rejects subscriptions, batching, executable extensions,
compressed requests and unsupported protocols. Conditions and policy masks can
skip the plugin; configure them deliberately. The ordinary upstream response
continues streaming.

Local HTTP rejections use the [GraphQL JSON error contract](graphql-errors.md).

## Root-field rules

Configure separate query and mutation root policies without requiring SDL:

```yaml
type: GraphqlGuard
config:
  rootFields:
    query:
      allow: [users, health, __typename]
      deny: [internalStatus]
    mutation:
      allow: [updateProfile]
```

Missing/null policies or allow lists are unrestricted. An empty allow list denies
all roots; deny wins even when the same name appears in allow. Each list accepts
at most 256 unique GraphQL names, each 1..256 bytes. Names are checked syntactically;
the backend remains responsible for full schema validation and authorization.

Every selected root occurrence must pass. Aliases use the underlying field name,
fragments retain semantic root depth, and skip/include branches are conservatively
checked regardless of their variable/literal values. Nested fields are outside
these root policies. The selected operation's verified kind determines which
policy applies. __typename is checked like other roots; allow entries cannot
override allowIntrospection=false. Root-policy denials return 403; shape/cost and
introspection violations retain 400. Optional SDL costing is unchanged.

This remains a trusted-chain inspection snapshot. The existing whole-resource
DSL/WAF retention policy may preserve earlier root permissions when a valid
restriction is bundled with invalid DSL/WAF changes. Put GraphqlGuard in a separate
EdgionPlugins resource when these permission updates must be independent.
