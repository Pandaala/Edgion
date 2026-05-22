---
name: gateway-http-route-engine
description: HTTP route matching engine — RadixPath, RegexSet, ConfHandler, proxy implementation, LB policy sync.
---

# HTTP route engine

## Module structure

```
src/core/gateway/routes/http/
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
├── lb_policy_sync.rs           # sync LB policies into the global PolicyStore
└── tests.rs
```

## Radix Path matching

`RadixRouteMatchEngine` implements Exact and Prefix path matching via an immutable radix tree:

1. **Build phase** (`build`): iterate every `HttpRouteRuleUnit`, create a `RadixPath` per path, insert into `RadixTreeBuilder`, and finally `freeze()` into an immutable `RadixTree`.
2. **Match phase** (`match_route`):
   - Call `tree.match_all_ext(path)` to obtain all candidates and their `MatchKind` in one pass.
   - Filter: Exact accepts only `FullyConsumed`; Prefix accepts `FullyConsumed` or `SegmentBoundary`.
   - Sort by `priority_weight` descending; on ties, run the full Gateway API priority chain via `MatchSpecificity::cmp_precedence()` (method → headers → query params → timestamp → name → rule/match index).
   - Run `deep_match` on candidates in turn; the first that passes is the final match.

Core fields of `RadixPath`:

| Field | Description |
|-------|-------------|
| `original` | Original path pattern |
| `normalized` | Path normalization (collapse consecutive slashes) |
| `priority_weight` | `path_char_count * 4 + type_bonus` |
| `is_prefix_match` | Whether the match is a prefix |
| `has_params` | Whether it contains a `:param` parameter segment |
| `segment_count` | Number of path segments |

The radix tree internally supports `:param` parameter matching; a double colon `::` is escaped as a literal colon.

## Regex route matching

`RegexRoutesEngine` performs efficient batch regex matching using `regex::RegexSet`:

- All regex patterns are compiled into a single `RegexSet`; match complexity is O(M) where M is the input path length.
- Routes are sorted by pattern string length descending; longer (more specific) patterns are preferred.
- When `RegexSet` construction fails, falls back to linear matching one by one.
- The regex engine runs before the radix engine inside `RouteRules.match_route`.

## proxy_http implementation

`EdgionHttpProxy` implements Pingora's `ProxyHttp` trait. Key callbacks during the request lifecycle:

| Callback | File | Responsibility |
|----------|------|----------------|
| `new_ctx` | `pg_new_ctx.rs` | Create `EdgionHttpContext` |
| `early_request_filter` | `pg_early_request_filter.rs` | Protocol detection (gRPC / gRPC-Web), request info extraction |
| `request_filter` | `pg_request_filter.rs` | Route matching, plugin execution, gRPC route attempt |
| `upstream_peer` | `pg_upstream_peer.rs` | Backend selection, BackendTLSPolicy lookup, connection establishment |
| `request_body_filter` | `pg_request_body_filter.rs` | Request body filtering |
| `upstream_response_filter` | `pg_upstream_response_filter.rs` | Upstream response header processing |
| `upstream_response_body_filter` | `pg_upstream_response_body_filter.rs` | Upstream response body processing |
| `response_filter` | `pg_response_filter.rs` | Downstream response filtering |
| `logging` | `pg_logging.rs` | Access log recording |
| `fail_to_connect` | `pg_fail_to_connect.rs` | Connect-failure handling |
| `error_while_proxy` | `pg_error_while_proxy.rs` | Errors during proxy |

## Redirect handling

`EdgionHttpRedirectProxy` is a lightweight `ProxyHttp` implementation that handles HTTP→HTTPS redirects:

- Enabled via the Gateway annotation `edgion.io/http-to-https-redirect: "true"`.
- Returns status code 301 with a `Location` header pointing to the HTTPS URL.
- Port handling: when the HTTPS port is 443, the port number is omitted.

## LB policy sync

`lb_policy_sync.rs` extracts LB policies and syncs them to the global `PolicyStore` when routes update:

- `sync_lb_policies_for_routes`: iterate all HTTPRoute rules' `backend_refs`, extract `extension_info.lb_policy`, and update `PolicyStore`.
- `cleanup_lb_policies_for_routes`: remove LB policies associated with deleted routes.
- Supported policy types: `LBPolicyConsistentHash` (header.xxx / cookie.xxx / arg.xxx), `LBPolicyLeastConn`.

## conf_handler_impl flow

`ConfHandler<HTTPRoute>` is implemented in two modes — full and incremental:

### full_set

1. Sync LB policies.
2. Clear `route_cache` and replace fully.
3. Call `rebuild_all_listener_managers` to rebuild every listener route table.

### partial_update

1. Compute the set of affected listeners (from changed routes' `resolved_listeners`).
2. Merge add+update, sync LB policies, clean up LB policies for removed routes.
3. Update `route_cache`.
4. Call `rebuild_affected_listener_managers` to rebuild only affected listeners.

### Route parsing

`parse_http_routes_to_domain_rules` parses HTTPRoutes into a `DomainRouteRulesMap`:

1. Validate route validity (non-empty parent_refs, rules, namespace, name).
2. `filter_accepted_parent_refs`: keep parentRefs whose status is `Accepted=True`.
3. `get_effective_hostnames`: prefer `resolved_hostnames` (the Controller-precomputed intersection), fall back to `spec.hostnames`, then to `"*"` (catch-all).
4. For each hostname × rule × match combination, split into a regex route or a normal route.
5. Precompile header / query RegularExpression regexes for each match_item.

`build_domain_route_rules_from_routes` builds the parsed result into the final data structures: exact domains go into `exact_domain_map`, wildcard domains build a `RadixHostMatchEngine`, and routes without hostname become `catch_all_routes`.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/http/routes_mgr.rs` | GlobalHttpRouteManagers, RouteRules, DomainRouteRules |
| `src/core/gateway/routes/http/conf_handler_impl.rs` | ConfHandler implementation, route parsing |
| `src/core/gateway/routes/http/match_engine/radix_route_match.rs` | RadixRouteMatchEngine |
| `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs` | RegexRoutesEngine |
| `src/core/gateway/routes/http/match_engine/radix_path.rs` | RadixPath priority calculation |
| `src/core/gateway/routes/http/match_unit.rs` | HttpRouteRuleUnit, deep_match |
| `src/core/gateway/routes/http/proxy_http/mod.rs` | EdgionHttpProxy |
| `src/core/gateway/routes/http/redirect_http.rs` | EdgionHttpRedirectProxy |
| `src/core/gateway/routes/http/lb_policy_sync.rs` | LB policy sync |
