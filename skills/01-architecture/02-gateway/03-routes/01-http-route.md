---
name: gateway-http-route-engine
description: HTTP route matching engine — RadixPath, RegexSet, ConfHandler, proxy implementation, session persistence.
---

# HTTP route engine

## Module structure

```
edgion-gateway/src/routes/http/
├── conf_handler_impl.rs        # ConfHandler<HTTPRoute> implementation, route parsing and DomainRouteRules construction
├── match_engine/
│   ├── mod.rs
│   ├── radix_path.rs           # RadixPath: path pattern compilation, priority computation
│   ├── radix_route_match.rs    # RadixRouteMatchEngine: radix tree match engine
│   └── regex_routes_engine.rs  # RegexRoutesEngine: RegexSet batch regex matching
├── match_unit.rs               # HttpRouteRuleUnit: route match unit, deep-match logic
├── routes_mgr.rs               # RouteRules / DomainRouteRules / GlobalHttpRouteManagers
├── proxy_http/                 # Pingora ProxyHttp trait implementation
│   ├── mod.rs                  # EdgionHttpProxy main struct
│   ├── pg_request_filter.rs    # request filter and route matching entry
│   ├── pg_upstream_peer.rs     # backend selection and connection
│   ├── pg_response_filter.rs   # response filter
│   └── ...                     # other Pingora callbacks
├── redirect_http.rs            # EdgionHttpRedirectProxy: HTTP→HTTPS 301 redirect
├── session_persistence.rs      # Gateway API SessionPersistence: cookie/header session affinity
└── tests.rs
```

## Radix Path matching

`RadixRouteMatchEngine` implements Exact and Prefix path matching via an immutable radix tree:

1. **Build phase** (`build`): iterate every `HttpRouteRuleUnit`, create a `RadixPath` per path, insert into `PathRadixBuilder`, `freeze()` into an immutable `PathRadixTree`, and finally rank every candidate once (`assign_precedence_ranks`) into `RadixPath::precedence_rank`.
2. **Match phase** (`match_route`):
   - Call `tree.match_all_ext(path)` to obtain all candidates and their `PathMatchKind` in one pass.
   - Filter: Exact accepts only `FullyConsumed`; Prefix accepts `FullyConsumed` or `SegmentBoundary`.
   - Order candidates by the build-time `precedence_rank` (a plain integer sort). The rank encodes `PathSpecificity` descending, then the full Gateway API priority chain via `MatchSpecificity::cmp_precedence()` (method → headers → query params → timestamp → name → rule/match index), then `path_idx`.
   - Run `deep_match` on candidates in turn; the first that passes is the final match.

Core fields of `RadixPath`:

| Field | Description |
|-------|-------------|
| `normalized` | Path normalization (collapse consecutive slashes; remove the non-root PathPrefix trailing slash) |
| `specificity` | Exact, static segment count, decoded static character count, total segment count; parameter names excluded |
| `is_prefix_match` | Whether the match is a prefix |
| `precedence_rank` | Global candidate order, assigned in the last step of the build phase; 0 = highest priority |

The radix tree internally supports `:param` parameter matching; a double colon `::` is escaped as a literal colon.

At configuration build time, resource preprocessing and `RadixPath::new` share
`canonicalize_path_prefix` after their existing normalization. PathPrefix `/abc/`
and `/abc` use the same tree key and path specificity; non-path ties decide their
order. Root `/`, Exact trailing slashes, and request URI semantics are preserved.

## Logical listener matching

Before selecting a logical Gateway, the shared deep matcher checks each resolved parent's
namespace/name, optional section and optional declared port against the current listener index.
Section and port constraints are conjunctive. The per-route physical listener-key union is only
a bucketing aid, not authorization for another parent on a shared socket. See the
[runtime parent-port guard](../../../04-review/routing/regression-guards/runtime-parent-port-eligibility.md).

## Regex route matching

`RegexRoutesEngine` performs efficient batch regex matching using `regex::RegexSet`:

- All regex patterns are compiled into a single `RegexSet`; match complexity is O(P * M), where P is the combined pattern-set size and M is the input path length.
- Routes are sorted by pattern string length descending; longer (more specific) patterns are preferred.
- When `RegexSet` construction fails, falls back to linear matching one by one.
- `RouteRules.match_route` tries the radix Exact/PathPrefix engine first and uses the regex engine
  only as a fallback, preserving `Exact > PathPrefix > RegularExpression`.

## Delegated parent path constraints (ARCH-156)

Rule compilation validates `resolvedParentMatch` before building plugins.
Resolved rules require explicit `direct` or `delegated` provenance. Delegated
scope counts must match effective match counts; each scope contains 1..8 absolute
ancestor prefixes. Each candidate holds an immutable shared compiled radix tree
from its own rule generation. These prefixes are already BASE-normalized by the
Controller; Gateway must not BASE-normalize them again. Canonical `RadixPath`
trailing-slash handling and the shared radix parser preserve segment boundaries,
parameters, and escaped colons.

`deep_match` checks every ancestor against `ctx.path()` before choosing a winner,
for radix, RegexSet, and linear fallback alike. Failure continues selection.
Inherited header/query/method predicates are already merged and are not duplicated.
At most eight inserted IDs bound both inline match results (capacity eight) and
DFS frontier (capacity sixteen); this guard adds no request heap allocation,
locks, regex compilation, tracing, or global parent lookup. It introduces no
new store or independently published state. Old candidates keep their own guards
across reload. Typed-valid malformed scopes withdraw the affected effective rule;
raw serde failures retain the existing transport error boundary.

## proxy_http implementation

`EdgionHttpProxy` implements Pingora's `ProxyHttp` trait. Key callbacks during the request lifecycle:

| Callback | File | Responsibility |
|----------|------|----------------|
| `new_ctx` | `pg_new_ctx.rs` | Create `EdgionHttpContext` |
| `early_request_filter` | `pg_early_request_filter.rs` | Protocol detection (gRPC / gRPC-Web), request info extraction |
| `request_filter` | `pg_request_filter.rs` | Route matching, plugin execution, gRPC route attempt |
| `upstream_peer` | `pg_upstream_peer.rs` | Backend selection (incl. session-affinity sticky target), BackendTLSPolicy lookup, connection establishment |
| `request_body_filter` | `pg_request_body_filter.rs` | Request body filtering |
| `upstream_request_filter` | `pg_upstream_request_filter.rs` | Outgoing upstream request header rewrite (e.g. `grpc-timeout`) |
| `upstream_response_filter` | `pg_upstream_response_filter.rs` | Upstream response header processing (incl. session-affinity `Set-Cookie`) |
| `upstream_response_body_filter` | `pg_upstream_response_body_filter.rs` | Upstream response body processing |
| `response_filter` | `pg_response_filter.rs` | Downstream response filtering |
| `logging` | `pg_logging.rs` | Access log recording |
| `fail_to_connect` | `pg_fail_to_connect.rs` | Connect-failure handling |
| `error_while_proxy` | `pg_error_while_proxy.rs` | Errors during proxy |

The proxy retains only the listener facts needed on the request path: the
listener port and whether the listener was constructed as HTTPS. It does not
retain the full `Listener` resource or unused GatewayClass/start-time metadata.
Application-controlled Pingora subrequest spawning stays at the upstream
default of disabled. Pingora's internal cache subrequests and RequestMirror's
separate bounded outbound request path are unaffected by that setting.

## Redirect handling

`EdgionHttpRedirectProxy` is a lightweight `ProxyHttp` implementation that handles HTTP→HTTPS redirects:

- Enabled via the Gateway annotation `edgion.io/http-to-https-redirect: "true"`.
- Returns status code 301 with a `Location` header pointing to the HTTPS URL.
- Port handling: when the HTTPS port is 443, the port number is omitted.
- Redirects and Host-validation 400 responses use Pingora's normal session
  keepalive policy and generated `Connection` header. They do not force closure;
  a client-requested close is respected. See the
  [connection-policy decision](../../../04-review/redirect/intentional-design/session-owned-connection-policy.md).

## LB policy

The route no longer carries an LB policy. The route-level `LBPolicy*`
`ExtensionRef` filter mechanism and its per-route sync module
(`lb_policy_sync.rs`, the route-fed `PolicyStore`) were removed; a route
`backendRef` filter referencing an `LBPolicy*` kind now fails validation like
any other unrecognized `extensionRef`. The LB algorithm is resolved purely
from the target Service key at selection time, via the
`EdgionBackendTrafficPolicy` policy store — see
[06-load-balancing.md](../06-load-balancing.md) and
[07-backends.md](../07-backends.md#backend-traffic-policy-edgionbackendtrafficpolicy).

## Session persistence

`session_persistence.rs` implements Gateway API `SessionPersistence` (cookie- or header-based session affinity):

- **Token derivation**: `generate_session_token(backend_addr)` returns a deterministic 16-char hex token (SHA-256 of the backend `ip:port` truncated to 64 bits). It is a stickiness hint, not a security boundary (no MAC/secret) — see the module doc-comment threat model. `build_sp_affinity_map` reuses this function so map keys and injected tokens can never drift.
- **Default name**: `edgion-session-affinity` (overridable via the resource's `sessionName`). `get_session_name` / `get_persistence_type` read the config.
- **Request path** (`pg_upstream_peer.rs` / `grpc/integration.rs`): `sp_extract_token` reads the token from the cookie or header; `sp_resolve_target_rule_scoped` then resolves it **before** weighted backendRef selection, across ALL of the matched rule's backendRefs in declaration order (weight-0 refs included — GEP-1619: weights govern only new sessions). A hit selects that backendRef and pins the endpoint (`sp_target_addr`, with SSRF validation on the upstream-peer fallback path); a full miss falls back to weighted RR and refreshes the token. An `Internal` upstream jump wins over the sticky session for that request without rewriting the session.
- **Response path** (`pg_upstream_response_filter.rs`): `build_set_cookie_value` emits the `Set-Cookie` for cookie-mode persistence.

## conf_handler_impl flow

`ConfHandler<HTTPRoute>` is implemented in two modes — full and incremental:

### full_set

1. Clear `route_cache` and replace it fully.
2. Call `rebuild_all_listener_managers` to rebuild every listener route table.

### partial_update

1. Compute the set of affected listeners (from changed routes' `resolved_listeners`).
2. Merge add+update and update `route_cache`; remove deleted route keys.
3. Call `rebuild_affected_listener_managers` to rebuild only affected listeners.

LB policy is not synchronized from routes. It is resolved from the target Service's
`EdgionBackendTrafficPolicy` at backend selection time.

### Route parsing

`parse_http_routes_to_domain_rules` parses HTTPRoutes into a `DomainRouteRulesMap`:

1. Validate route validity (non-empty parent_refs, rules, namespace, name).
2. `filter_accepted_parent_refs` preserves Accepted-status filtering and existing
   missing-status optimism; delegation parent kinds cannot attach to Gateways.
3. Validate the complete authored hostname list. `get_declared_hostnames` uses
   `spec.hostnames`; only omitted/empty lists create the internal catch-all.
4. Compile one rule runtime/backend selector and share match-unit Arcs across
   declared hostname groups; Listener eligibility remains in deep matching.
5. Precompile header / query RegularExpression regexes for each match_item.

`build_domain_route_rules_from_routes` stores all named groups in the gateway's
`RadixHostnameMatcher<RouteRules>`, with hostless fallback separate. Exact and
descending wildcard groups continue only on RouteNotFound, not other errors.
Listener intersections never promote wildcard/hostless Route specificity.
Route deltas rebuild touched declared-hostname groups, removing all old units of
changed/deleted resource keys before adding new units; untouched groups retain
their path/regex engines. Full-list and Gateway rebuilds can reconstruct payloads
but reuse the old hostname tree when final keys agree. Each outer table atomically
couples that tree with its content map; no per-host ArcSwap or cross-listener transaction.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/http/routes_mgr.rs` | GlobalHttpRouteManagers, RouteRules, DomainRouteRules |
| `edgion-gateway/src/routes/http/conf_handler_impl.rs` | ConfHandler implementation, route parsing |
| `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs` | RadixRouteMatchEngine |
| `edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs` | RegexRoutesEngine |
| `edgion-gateway/src/routes/http/match_engine/radix_path.rs` | RadixPath priority calculation |
| `edgion-gateway/src/routes/http/match_unit.rs` | HttpRouteRuleUnit, deep_match |
| `edgion-gateway/src/routes/http/proxy_http/mod.rs` | EdgionHttpProxy |
| `edgion-gateway/src/routes/http/redirect_http.rs` | EdgionHttpRedirectProxy |
| `edgion-gateway/src/routes/http/session_persistence.rs` | Gateway API SessionPersistence |
