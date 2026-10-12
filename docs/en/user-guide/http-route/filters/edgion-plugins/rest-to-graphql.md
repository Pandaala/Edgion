# REST-to-GraphQL

`RestToGraphql` exposes an existing GraphQL upstream through ordinary HTTPRoute
endpoints. Configure the document and approved variable bindings; clients provide
values rather than choosing the query or returned fields. Responses keep the
upstream GraphQL `data`/`errors` envelope and stream normally.

## Example

The HTTPRoute selects `/users` and the declared backend. CtxSet extracts the user
identifier, and the adapter turns `GET /users/42?n=3` into a JSON POST to `/graphql`.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: user-graphql
  namespace: default
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: user_id
            from: { type: path }
            extract: { regex: '^/users/([^/]+)$', group: 1 }
    - type: RestToGraphql
      config:
        query: |
          query User($id: ID!, $n: Int! = 2) {
            user(id: $id, limit: $n) { id name }
          }
        operationName: User
        path: /graphql
        variables:
          - name: id
            from: { type: ctx, name: user_id }
          - name: n
            from: { type: query, name: n }
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: user-rest
  namespace: default
spec:
  parentRefs:
    - name: public-gateway
  hostnames: [users.example.com]
  rules:
    - matches:
        - path: { type: PathPrefix, value: /users }
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: user-graphql
      backendRefs:
        - name: user-graphql-service
          port: 8080
```

The referenced Gateway and Service must exist. The upstream Service must already
serve GraphQL; this plugin does not generate SQL or a search-engine query.

## Configuration

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `query` | string | required | Static query/mutation document, at most16KiB; selected through the canonical GraphQL parser |
| `operationName` | string | omitted | Operation selector; required when the document contains multiple operations |
| `path` | string | `/graphql` | Upstream origin-form path, at most2048 bytes; query, fragment and authority are forbidden |
| `variables` | list | `[]` | At most64 unique mappings to variables declared by the selected operation |
| `variables[].name` | string | required | Destination GraphQL variable name |
| `variables[].from` | KeyGet | omitted | Shared local source, such as ctx/header/query/path; exclusive with bodyPointer |
| `variables[].bodyPointer` | string | omitted | RFC6901 pointer into the current strict JSON body; exclusive with from; empty pointer selects the whole body |
| `variables[].format` | `text` / `json` | `text` | String-source interpretation; bodyPointer supplies native JSON and requires text |

Simple bindings use KeyGet directly. Use preceding CtxSet/DSL for constants,
extraction, transformations or context preparation. KeyGet `path` returns the
current complete normalized path, not a named path parameter. Query bindings
observe the original entry query; header/context/body bindings observe the
adapter-time state. Path extraction timing follows the preceding CtxSet/DSL.

`String`/`ID` text remains a string. `Int`, `Float`, `Boolean` text uses strict JSON
scalar syntax; Int is signed32-bit. Lists use JSON arrays (including from a text
source); scalar-to-list coercion is unsupported. Explicit `format: json` allows
JSON objects or quoted JSON strings from ctx/header/query. Body pointers retain
native JSON types. All variable wrapper nullability is checked. Custom enum,
input-object and scalar definitions, full SDL validation and static default
semantics are checked by the upstream GraphQL service.

Missing nullable/defaulted variables are omitted so upstream defaults apply.
Missing non-null variables without defaults, and explicit null for a non-null
variable, reject. Required variables without defaults need a configured mapping.
Values are limited to16KiB of profiled text,4096 nodes/depth32 and32 elements per
array. Variable type wrappers have at most8 levels. The complete generated
envelope retains the canonical GraphQL64KiB/depth/node/parser/work bounds.

## Request and protection order

The original client method must be GET/POST/PUT/PATCH/DELETE. GET cannot execute
a mutation, including after an earlier method rewrite, and cannot carry a physical
body. Generated requests use POST and application/json; all REST query parameters
are consumed. Identity Content-Encoding is required. A nonempty input body
requires supported JSON media and strict JSON parsing even when unused.
Duplicate decoded query names, malformed query encodings and repeated/non-UTF-8
mapped header values reject. Entry query is bounded before KeyGet lookup.

The adapter runs in the request stage, before Guard/Safelist/RateLimit/Router.
Global plugins execute before Gateway plugins, then Route plugins. Successful
GraphQL inspection freezes the canonical request body, including a replay owner
installed later for an originally bodyless request. A late adapter therefore
cannot publish a replacement body. Failed or skipped inspections do not freeze;
multiple adaptations before final inspection are allowed. Put authentication and
REST preparation first, adapters next, then GraphQL protections. Body mutation
through DSL/ProxyRewrite/Wasm after successful inspection is refused by the shared
body owner. Query/URI/method/header changes remain subject to the trusted-chain
restriction; this is not a general transport seal.

GraphqlOperationRouter still selects only backendRefs declared by the attaching
HTTPRoute rule. The adapter owns neither backend selection nor response buffering.
The shared request BodyStorage/replay and upstream framing owner handle the
generated body, including originally bodyless requests and retries.

## Failures and reloads

Adapter runtime rejections use the [fixed GraphQL JSON errors](graphql-errors.md). Invalid
mapping/input returns400; method violations405; caps413; unsupported media/encoding415;
evaluation admission exhaustion503; invalid order/publication returns500.
Invalid enabled configuration uses the canonical plugin-configuration500 response.
Rejections terminate before upstream attempts. Disabled entries are inert;
invalid enabled updates fail closed and valid updates recover. An immutable
generation remains pinned for requests already using it.
