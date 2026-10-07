# GraphQL Rate Limit

`GraphqlRateLimit` charges schema-aware query costs against Redis budgets before
proxying a GraphQL request. It protects a GraphQL service which may call databases
such as Elasticsearch or ClickHouse. Direct Elasticsearch DSL and ClickHouse SQL
requests need their own policies; this plugin analyzes GraphQL.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: graphql-budget
spec:
  requestPlugins:
    - type: GraphqlRateLimit
      config:
        schema: |
          type Query { users(first: Int = 10): [User!]! }
          type User { name: String! }
        costRules:
          - field: Query.users
            weight: 1
            multipliers: [first]
            defaultMultiplier: 100
        maxCost: 10000
        maxDepth: 20
        redisRef: default/shared-redis
        policies:
          - rate: 10000
            interval: 1m
            algorithm: SlidingWindow
```

Attach the resource through an HTTPRoute ExtensionRef. Redis uses an existing
LinkSys Redis resource, as described by [RateLimitRedis](rate-limit-redis.md). For composition see
[execution order](../plugin-composition.md).

## Cost and configuration

The operation costs one unit plus the selected fields. Each field costs
`weight + product(multipliers) * sum(child costs)`. An unconfigured field costs
one plus its children. `{ users(first: 10) { name } }` costs12 units with the
example schema; omitting `first` also costs12 because the SDL default is10.

Weights and pagination defaults are estimates chosen by the operator. SDL alone
cannot reveal resolver fanout, database work or actual CPU use. Configure
multipliers for resolvers returning objects, interfaces or unions, including lists
of those types, and match their real pagination defaults. Scalar/enum returns,
including scalar/enum lists, have no child costs: nonempty `multipliers` are
rejected for these fields. Use a conservative fixed `weight` instead.
Fields are identified by `Type.field`, regardless of aliases. Repeated fields and
fragment spreads count per occurrence. `@skip`/`@include` branches are always
counted. Interfaces/unions use the maximum over possible concrete objects, and
each concrete field applies the most expensive complete concrete/interface rule.
These conservative choices may overcharge compared with actual execution.

| Field | Default / restriction |
|-------|-----------------------|
| `schema` | Required inline SDL; no schema discovery or retrieval |
| `costRules` | Empty; at most256 rules |
| `costRules[].field` | Required existing object/interface `Type.field` |
| `costRules[].weight` | 1; range1..1,000,000 |
| `costRules[].multipliers` | Empty; up to8 distinct scalar Int argument names; nonempty only for object/interface/union returns |
| `costRules[].defaultMultiplier` | 100; used when no argument/variable/SDL default exists |
| `maxCost` | 10,000; range1..1,000,000 |
| `maxDepth` | 20; range1..32 |
| `redisRef`, `policies` | Required; same policy types as [RateLimitRedis](rate-limit-redis.md) |
| `key` | `[{type: remoteIp}]`; an empty list uses one global budget |
| `onRedisFailure` | `Deny`; explicit `Allow` permits valid queries during Redis failure |
| `requestTimeout` | `200ms`; positive duration for the Redis check |
| Entry `body.maxBodySize` | 64KiB; may lower the POST and GET limits |

All-missing identity sources deny; partially missing composite sources keep the
canonical placeholders. Use authenticated keys and place their producer before
this plugin. Buckets are scoped by plugin resource and entry identity: routes
sharing that entry share its budget, across Gateway replicas. A request is charged
once before backend selection; backend failure does not refund it. Multiple
policies are individually atomic and can debit independently.

## Requests and placement

POST accepts one JSON object containing a nonempty `query`, optional string/null
`operationName`, object/null `variables`, and empty object/null `extensions`.
Its URI must have no nonempty query string. Require one UTF-8 JSON Content-Type
and absent/identity Content-Encoding. GET accepts the same parameters in strictly
decoded URL form, has no body, and can execute only a query. Multiple operations
require `operationName`. Duplicate keys/parameters and unknown envelope parameters
are rejected.

Variables override operation defaults, then the actual concrete SDL argument
defaults apply. Missing non-null cost variables, explicit null, strings, negative
and fractional values reject. JSON integral numbers such as1.0 are accepted as
integers; GraphQL float literals are rejected for cost multipliers. Multiplier
values are0..1,000,000 and zero is charged as one.

The effective envelope cap applies separately to POST body bytes, encoded GET
query bytes and decoded GET parameter bytes. SDL and query text also have fixed
token/recursion/work limits:4096 tokens including trivia,256 bytes per nontrivia
token, and32768 evaluation steps. Put longer application strings in variables.
SDL extensions, directives and interface inheritance are unsupported. The plugin
validates its cost model; the backend still owns full GraphQL validation/execution.
Subscriptions, WebSocket messages, batch arrays, uploads, compressed bodies and
persisted-only queries are unsupported.

Place this plugin after authentication and every rewrite of the GraphQL request,
in global/Gateway/route request plugins. BackendRef late placement fails closed.
This is a cost snapshot at its execution position: later request plugins,
route/backend filters, DSL or ExtProc must not alter the query, body, method or
representation. Edgion does not seal subsequent rewrites. Entry conditions,
disabled entries and plugin policies can skip the complete protection.
`body.conditions` and `body.onReadFailure: failOpen` are invalid.

## Outcomes

| Condition | Status |
|-----------|--------|
| Valid request within budget | Proxy unchanged; X-RateLimit headers report cost units |
| Malformed/unsupported query, depth/work/maxCost violation | 400 |
| Unsupported method or GET mutation | 405 |
| Envelope over limit | 413 |
| Unsupported media/encoding | 415 |
| Budget exhausted, all identity keys missing, or default Redis Deny | 429 |
| Single cost exceeds a policy capacity | 429 without Retry-After or Redis debit |
| Process evaluation admission exhausted | 503 |
| Invalid configuration, late placement, internal evaluation failure | 500 |

Invalid enabled configuration reports `Accepted=False` and fails traffic closed,
including after hot reload. A valid update restores the new immutable cost model.
Disabled entries remain inert. The plugin introduces no response aggregation or
execution engine. Its logs omit request text and variables; existing optional
`access_log_options.includeRawUri=true` can expose a GET query in access logs.
