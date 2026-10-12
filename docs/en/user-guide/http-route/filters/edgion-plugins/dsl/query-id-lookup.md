# Authenticated query-ID lookup with DSL

This application recipe uses ordinary HTTPRoute, Secret-backed KeyAuth, DSL and
LinkSys Redis. An ID locates a query document; it grants no permission and selects
no upstream URL. It is not Apollo APQ, a built-in registry or automatic safelist.

## Prerequisites and complete configuration

Use an existing Gateway named `app-gateway` in namespace `example`, serving
`graphql.example.com` over TLS. Deploy your GraphQL application with label
`app: graphql-api` on port 8080. Supply a reachable Redis endpoint and three
operator-provisioned credentials in the configuration below. Replace the example
password and API keys; publisher and executor privileges are separate.

Both execution paths deliberately share one HTTPRoute rule, backend and plugin
chain. Expansion precedes GraphqlGuard and ResponseCache. Place every Global or
Gateway GraphQL/body inspector after expansion too; an earlier successful
inspection freezes the body and reconstruction fails closed. Avoid introducing
such an earlier inspector for these routes.

The literal Redis prefix fixes application scope. Unverified tenant headers and
submitted namespaces never select another scope. Grant publisher credentials
only to deployment/CI actors allowed to publish documents. Give executor keys to
callers authorized for this application. Signature verification normally precedes
rewrites and must verify the request view it signs. Additional document-specific
authorization, Safelist, RateLimit or Router runs after expansion and before cache;
existing request quotas are charged on hits as well as misses.

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: example
---
apiVersion: v1
kind: Secret
metadata:
  name: query-redis-auth
  namespace: example
type: Opaque
stringData:
  password: replace-with-redis-password
---
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: redis-test
  namespace: example
spec:
  type: redis
  config:
    endpoints:
    - redis://redis.example.svc:6379
    auth:
      secretRef:
        name: query-redis-auth
    timeout:
      connect: 3s
      command: 2s
---
apiVersion: v1
kind: Service
metadata:
  name: graphql-api
  namespace: example
spec:
  selector:
    app: graphql-api
  ports:
  - name: http
    port: 8080
    targetPort: 8080
---
apiVersion: v1
kind: Secret
metadata:
  name: query-publisher
  namespace: example
type: Opaque
stringData:
  keys.yaml: |
    - key: "replace-with-publisher-key"
---
apiVersion: v1
kind: Secret
metadata:
  name: query-executor
  namespace: example
type: Opaque
stringData:
  keys.yaml: |
    - key: "replace-with-executor-key"
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: query-publisher
  namespace: example
spec:
  requestPlugins:
  - type: KeyAuth
    config:
      keySources:
      - type: header
        name: x-dsl-query-key
      hideCredentials: true
      upstreamHeaderFields: []
      secretGroups:
      - name: default
        secretRefs:
        - name: query-publisher
          namespace: example
  - type: Dsl
    config:
      name: query-publisher
      source: |
        if req.request_method != "POST" { return deny(405, "Method not allowed") }
        if req.content_type != "application/json" { return deny(415, "Unsupported media type") }
        let encoding = req.header("Content-Encoding")
        if encoding != nil && encoding != "identity" { return deny(415, "Unsupported media type") }
        if req.query_string != nil && req.query_string != "" { return deny(400, "Invalid input") }
        if req.body.byte_len() > 65536 { return deny(413, "Payload too large") }
        let input = json.read(req.body)
        if input == nil { return deny(400, "Invalid input") }
        let keys = json.keys(input)
        if input == nil || keys == nil { return deny(400, "Invalid input") }
        for key in keys {
          if key != "query" && key != "operationName" { return deny(400, "Invalid input") }
        }
        let query = json.get_string(input, "query")
        if query == nil || query == "" { return deny(400, "Invalid input") }
        let selection = json.get_raw(input, "operationName")
        let name = json.get_string(input, "operationName")
        if selection != nil && (name == nil || name == "") { return deny(400, "Invalid input") }
        let mut record = json.set_string("{}", "query", query)
        if selection != nil { record = json.set_string(record, "operationName", name) }
        let id = sha256(record)
        let result = redis.set("redis-test", "edgion:dsl:query:v1:example:" + id, record, 604800000)
        if result["ok"] == false { return deny(503, "Query store unavailable") }
        resp.set_header("Content-Type", "application/json")
        return deny(201, json.set_string("{}", "id", id))
      maxStringLen: 65536
      maxBodyScanPasses: 4
      errorPolicy: deny
      linkSysRefs:
      - namespace: example
        name: redis-test
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: query-executor
  namespace: example
spec:
  requestPlugins:
  - type: KeyAuth
    config:
      keySources:
      - type: header
        name: x-dsl-query-key
      hideCredentials: true
      upstreamHeaderFields: []
      secretGroups:
      - name: default
        secretRefs:
        - name: query-executor
          namespace: example
  - type: Dsl
    config:
      name: query-executor
      source: |
        if req.request_method != "POST" { return deny(405, "Method not allowed") }
        if req.content_type != "application/json" { return deny(415, "Unsupported media type") }
        let encoding = req.header("Content-Encoding")
        if encoding != nil && encoding != "identity" { return deny(415, "Unsupported media type") }
        if req.query_string != nil && req.query_string != "" { return deny(400, "Invalid input") }
        if req.body.byte_len() > 65536 { return deny(413, "Payload too large") }
        let input = json.read(req.body)
        if input == nil { return deny(400, "Invalid input") }
        let keys = json.keys(input)
        if input == nil || keys == nil { return deny(400, "Invalid input") }
        let by_id = req.path == "/graphql/by-id"
        for key in keys {
          if by_id {
            if key != "id" && key != "variables" { return deny(400, "Invalid input") }
          } else {
            if key != "query" && key != "operationName" && key != "variables" { return deny(400, "Invalid input") }
          }
        }
        let variables = json.get_raw(input, "variables")
        if variables != nil && variables != "null" && json.keys(variables) == nil { return deny(400, "Invalid input") }
        let mut record = "{}"
        if by_id {
          let id = json.get_string(input, "id")
          if id == nil || regex_find(id, "^[0-9a-f]{64}$") == nil { return deny(400, "Invalid input") }
          let result = redis.get_bounded("redis-test", "edgion:dsl:query:v1:example:" + id, 65536)
          if result["ok"] == false {
            if result["error"] == "result_too_large" || result["error"] == "invalid_response" { return deny(502, "Invalid query mapping") }
            return deny(503, "Query store unavailable")
          }
          if result["value"] == nil { return deny(404, "Query mapping not found") }
          if sha256(result["value"]) != id { return deny(502, "Invalid query mapping") }
          record = json.read(result["value"])
          if record == nil { return deny(502, "Invalid query mapping") }
          let stored_keys = json.keys(record)
          if record == nil || stored_keys == nil { return deny(502, "Invalid query mapping") }
          for key in stored_keys {
            if key != "query" && key != "operationName" { return deny(502, "Invalid query mapping") }
          }
          let query = json.get_string(record, "query")
          let selection = json.get_raw(record, "operationName")
          let name = json.get_string(record, "operationName")
          if query == nil || query == "" || (selection != nil && (name == nil || name == "")) { return deny(502, "Invalid query mapping") }
        } else {
          let query = json.get_string(input, "query")
          let selection = json.get_raw(input, "operationName")
          let name = json.get_string(input, "operationName")
          if query == nil || query == "" || (selection != nil && (name == nil || name == "")) { return deny(400, "Invalid input") }
          record = json.set_string("{}", "query", query)
          if selection != nil { record = json.set_string(record, "operationName", name) }
        }
        let mut envelope = record
        if variables != nil { envelope = json.set_raw(record, "variables", variables) }
        json.set_body(envelope)
        req.set_header("Content-Type", "application/json")
        req.set_uri("/graphql")
        return next()
      maxStringLen: 65536
      maxBodyScanPasses: 4
      errorPolicy: deny
      linkSysRefs:
      - namespace: example
        name: redis-test
  - type: GraphqlGuard
    config:
      maxDepth: 8
      maxFields: 64
      maxAliases: 16
      maxRootFields: 16
  - type: ResponseCache
    config:
      maxTtl: 60s
      graphqlQueries: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: query-publisher
  namespace: example
spec:
  parentRefs:
  - name: app-gateway
    namespace: example
  hostnames:
  - graphql.example.com
  rules:
  - matches:
    - path:
        type: Exact
        value: /graphql/operations
    filters:
    - type: ExtensionRef
      extensionRef:
        group: edgion.io
        kind: EdgionPlugins
        name: query-publisher
    backendRefs:
    - name: graphql-api
      port: 8080
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: query-executor
  namespace: example
spec:
  parentRefs:
  - name: app-gateway
    namespace: example
  hostnames:
  - graphql.example.com
  rules:
  - matches:
    - path:
        type: Exact
        value: /graphql
    - path:
        type: Exact
        value: /graphql/by-id
    filters:
    - type: ExtensionRef
      extensionRef:
        group: edgion.io
        kind: EdgionPlugins
        name: query-executor
    backendRefs:
    - name: graphql-api
      port: 8080
```

## Publish and execute

```bash
# Register with a publisher credential. Copy the returned id into QUERY_ID.
curl -i https://graphql.example.com/graphql/operations \
  -H 'x-dsl-query-key: replace-with-publisher-key' \
  -H 'Content-Type: application/json' \
  --data '{"query":"query User($id: ID!) { user(id: $id) { name } }","operationName":"User"}'

QUERY_ID='<returned-id>'
curl -i https://graphql.example.com/graphql/by-id \
  -H 'x-dsl-query-key: replace-with-executor-key' \
  -H 'Content-Type: application/json' \
  --data "{\"id\":\"${QUERY_ID}\",\"variables\":{\"id\":\"42\"}}"

curl -i https://graphql.example.com/graphql \
  -H 'x-dsl-query-key: replace-with-executor-key' \
  -H 'Content-Type: application/json' \
  --data '{"query":"query User($id: ID!) { user(id: $id) { name } }","operationName":"User","variables":{"id":"42"}}'
```

Registration returns 201 and an ID equal to SHA-256 of the stable stored JSON
bytes. The script decodes query and optional operationName, then constructs query
first and operationName second using canonical JSON setters. Input whitespace or
alternative string escapes do not alter document contents or its ID. A different
fixed selection creates a different ID. Variables belong to each invocation.

`operationName` may be absent; if present it must be a nonempty string. Null,
empty and non-string selections reject. A sole named or anonymous operation can
execute without a selection. Multiple operations without a fixed name reject in
canonical GraphqlGuard at execution. Publication validates JSON shape and limits,
not GraphQL syntax, SDL or backend semantics. Invalid documents and nonexistent
selections still reject before origin execution. By-ID invocation cannot override
operationName, even with the same name; mixed ID/query and unknown fields reject.

Absent variables stay absent, explicit null stays null, and object variables
preserve their original nested values and number lexemes. Strings are escaped by
json.set_string; json.set_raw preserves typed variables. Recursive duplicate keys,
including escaped aliases, reject. No GraphQL interpolation or manual JSON escaping
occurs. Exactly one atomic json.set_body publishes the complete envelope; Gateway
framing remains the sole owner of Content-Length and Transfer-Encoding.

## Expiry, revocation and failures

The Redis TTL is seven days: `604800000` milliseconds in redis.set. Publication
can reset TTL; reads never renew it. Delete the exact versioned prefix + ID key
through authorized Redis tooling to revoke a document. Lookup re-reads the live
mapping before every cache attempt, so a deleted, expired or corrupt mapping
cannot be rescued by a cached response. Credentials are also validated before
hits; deleting/replacing their Secret revokes access through ordinary hot reload.

| Condition | Result |
| --- | --- |
| Missing, wrong or cross-role credential | KeyAuth 401, before DSL or Redis |
| Original method other than POST | 405, even if a trusted plugin rewrites it |
| Unsupported media/encoding | 415 |
| Invalid JSON, duplicate/unknown fields, bad selection/ID/variables shape | 400 |
| Input beyond 65,536 bytes | 413 before JSON parsing |
| Missing/expired/revoked mapping | 404 |
| Corrupt, oversized or invalid UTF-8 mapping | Fixed 502 |
| Redis missing/unavailable/timeout | Fixed 503 |
| Structural/work limits or body publication failure | Existing fixed DSL/BodyAccess 500 |

Bodies must be POST application/json with absent/identity Content-Encoding and no
URL query parameters. Request, stored record and reconstructed output independently
obey the 64 KiB byte bound, depth 32 and 4,096 JSON nodes. Repeated parsing and
construction share the VM's 16 MiB work account, and physical body/scan limits
apply separately. A valid smaller document can still exhaust those limits.
redis.get_bounded never returns a truncated success; its conversion cap does not
bound Fred's prior Redis frame aggregation. Replies do not reflect stored queries,
variables, credentials or dependency errors.

## Response caching

The example enables [ResponseCache](../response-cache.md) only when your origin
explicitly returns finite public cacheable responses, for example
`Cache-Control: public, max-age=60`. The origin must not return personalized data
behind this public contract. Otherwise remove ResponseCache or mark responses
no-store. Credentials use the supported header-only KeyAuth profile and are
removed before origin; this is not an authenticated response-partition API.

Only canonically proven GraphQL queries are eligible; mutations, subscriptions,
streaming/incremental/SSE, unsupported body transforms, backend plugin chains,
Set-Cookie and private/no-store responses bypass caching. HTTP 200 alone does not
prove that a GraphQL response is error-free. The explicit origin contract controls
cacheability; no full-response JSON inspection is added.

Complete and ID requests share entries only when their final envelope, remaining
headers and route/backend/policy identity match. This script constructs both the
same way, but byte-different variable objects can miss. TTL is not immediate
business-data invalidation after a mutation; choose a short TTL or disable cache
when immediate freshness matters. Main upstream responses keep streaming; no DSL
response-body API or response BodyStorage is created.
