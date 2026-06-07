---
module: filter-plugin
title: No per-route override or exclusion of global plugins
severity: medium
envoy_reference: Envoy typed_per_filter_config and per-route filter disabling
---

# No per-route override or exclusion of global plugins

## Envoy's design (the proven baseline)

Envoy supports `typed_per_filter_config` at the VirtualHost, Route, and WeightedCluster levels.
This mechanism allows any global (listener-level) filter to be:

1. **Overridden per route** with a different configuration (e.g., a global rate limiter at 100
   req/s can be changed to 1000 req/s for a specific route).
2. **Disabled per route** entirely (a route can opt out of a global filter that would otherwise
   apply to all routes under the listener).
3. **Configured independently** at each level of the routing hierarchy without duplicating the
   full filter chain.

This is the mechanism used by RBAC, JWT auth, rate limiting, and CORS filters in production Envoy
deployments to avoid forcing operators to maintain N copies of a filter config when only M < N
routes need the variant.

## Edgion's current design

Global plugins are configured via `EdgionGatewayConfig.spec.global_plugins_ref`
(`src/types/resources/edgion_gateway_config.rs:49`). At request time,
`resolve_global_plugins` resolves this list once and stores the result in
`ctx.resolved_global_plugins`. The global plugin runtimes are then executed unconditionally
**before** all route-level plugins in `pg_request_filter.rs:196-207` and repeated in the
response phases (`pg_upstream_response_filter.rs:135-142`,
`pg_upstream_response_body_filter.rs:51-64`, `pg_response_filter.rs:50`).

There is no field on `HTTPRoute`, `GRPCRoute`, or any route rule that allows a route to:
- Override the configuration of a specific global plugin with a route-specific config.
- Disable a specific global plugin for that route only.

The only bypass available is Conditional execution on the global plugin itself (using
`conditions` / `skip` matching on headers, paths, etc.), but this requires the global plugin
author to predict all exclusion patterns up front and encode them into the plugin's config.
There is no route-side opt-out.

## The problem / gap

The absence of per-route override/disable leads to two concrete problems:

**Problem 1 — No selective exemption**: suppose a global `RateLimit` plugin applies 100 req/s
to all routes. An internal health-check route (`/healthz`) must be exempt. With Edgion, the
operator must bake a `conditions.skip` rule into the global `RateLimit` config, which couples
the rate-limiter's config to knowledge of all exempt routes. Adding a new exempt route requires
modifying the global plugin rather than the route. In Envoy, the route simply sets
`typed_per_filter_config: {envoy.filters.http.ratelimit: {disabled: true}}`.

**Problem 2 — No route-level config variant**: suppose a global `JwtAuth` plugin enforces JWT
for all routes. One route is a public webhook that must skip JWT. Another route is an admin API
that must use a different JWKS URL. Neither variant can be expressed today without either
duplicating the plugin into each route (defeating the purpose of global plugins) or encoding
complex condition logic into the single global plugin config.

The `conditions` mechanism partially mitigates exemption but is evaluated by the global plugin,
not the route — it provides path/header matching, not a first-class route-level override API.

## Impact

- Operators with a mixed public+private API surface are forced to choose between a global plugin
  that applies to everything (and encode all exceptions as conditions) or route-level plugins
  on every route (negating the operational value of global plugins).
- Any global security plugin (auth, WAF, IP restriction) that legitimately needs per-route
  variants requires ongoing coordination between the plugin config and the route config —
  a coupling that grows quadratically as routes multiply.
- The `disable_global_plugins: true` pattern used by internal liveness/readiness endpoints in
  production Envoy deployments has no equivalent, forcing workarounds like fake headers or IP
  allow-lists in condition expressions.

## Suggested direction

Add a per-route field — for example on `HTTPRouteRule` — that accepts a list of global plugin
exclusions or overrides:

```yaml
# proposed schema (not yet implemented)
rules:
  - matches: [...]
    globalPluginsOverride:
      excludePlugins:
        - name: rate-limiter        # disable this global plugin for this rule
      overridePlugins:
        - ref: {name: custom-jwt}   # replace the global jwt-auth with this one
```

At preparse time, the route builds its own "effective global plugin list" by merging the global
list with the rule's override/exclude entries. At runtime, `ctx.resolved_global_plugins` holds
the effective list (already per-request), so no hot-path change is needed beyond the preparse
merge step. This mirrors Envoy's per-route `typed_per_filter_config` semantics without requiring
a full Envoy-style filter factory system.
