# gw-plugin-05: Two independent CORS engines (EdgionPlugins `Cors` vs native Gateway API `CORS`) with divergent semantics

- **Category**: over-design
- **Severity**: P2
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/plugins/http/cors/plugin.rs` (295 lines) + `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs` (428 lines, `is_origin_allowed` at `:235`); `edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs` (302 lines, `is_origin_allowed` at `:114`); `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs:70-104`; `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:845-853`

## Summary
Edgion ships two complete CORS implementations: the EdgionPlugins `Cors` plugin (an APISIX-style
CSV string config with `*`/`**`, regex origins, timing-allow-origin, and private-network) and
`GapiCorsFilter` for HTTPRoute `type: CORS`. Each has its own origin matcher, preflight detection,
preflight response builder, actual-response header writer, and Vary policy, with different
semantics. Preflight routing (`preflight_handler.rs`) has to special-case the plugin. Every CORS
fix and review guard (13 regression guards under `skills/04-review/cors/`) has to be reasoned about
twice.

## Current behavior (evidence)
Behaviors that differ for the same intent:
- **Origin matching**: the plugin accepts an exact CSV cache plus a merged regex and rejects
  `"null"` (`cors.rs:235-265`). The native filter accepts prepared Exact, Any, and host-wildcard
  patterns with port equality (`cors_filter.rs:114-148`) and has no `"null"` rejection.
- **Actual-response header timing**: the plugin queues headers in the request stage through
  `set_response_header` (`cors/plugin.rs:173-188`). The native filter defers them to its
  `UpstreamResponseFilter` (`cors_filter.rs:272-299`).
- **Vary**: the plugin appends `Vary: Origin` conditionally (`should_add_vary_header`). The native
  filter appends it unconditionally after a match (`cors_filter.rs:296`). Neither adds it on the
  "origin not allowed" path, but that is decided in two places.
- **Preflight ownership**: `preflight_handler.rs:82` steps aside when a *plugin* `CorsConfig` is
  present. Native CORS is detected separately in `pg_request_filter` (see
  `skills/04-review/cors/intentional-design/native-cors-rule-scope.md`).

The denied-preflight 204 and credential-wildcard checks are re-implemented in both.

## Industry comparison
- **Envoy**: one `envoy.filters.http.cors` filter. Gateway API CORS in Envoy Gateway and Istio
  compiles into that single filter's `CorsPolicy`.
- **Traefik**: CORS is one set of `headers` middleware fields. Kong and APISIX have one `cors`
  plugin each.

None maintains two engines.

## Impact
- Maintenance and review cost doubles: two code paths, two config schemas, and two validation
  parities (`native-v15-validation-parity.md`, `token-list-validation.md`, …).
- Users get different answers for the same origin depending on which surface they chose. For
  example, a `null` origin, or `https://*.example.com:8443` versus a regex.
- Preflight dispatch has to know about both.

## Suggested direction
Make one CORS engine canonical. A compiled `PreparedCorsPolicy` with origin matcher, header
values, and Vary policy would be owned by the native filter or a shared module. Then:
- Compile `HTTPCORSFilter` into it directly.
- Compile the EdgionPlugins `Cors` config into the same policy. Keep only genuinely extra knobs
  (`timingAllowOrigins`, `allowPrivateNetwork`, regex origins) as optional fields of that policy.
  Or drop the plugin entirely now that Gateway API v1.5 CORS covers the core semantics; it is
  pre-release, so there is no compatibility burden.
- Use one preflight owner check and one Vary rule. Per Fetch, add `Vary: Origin` on every response
  whose ACAO depends on Origin, including the deny path when allow-origins is not `*`.

Recommendation: keep native `type: CORS` as the primary surface and fold the plugin's extras into
it as an Edgion extension of the same compiled policy.

## Notes
- `skills/04-review/cors/intentional-design/native-cors-rule-scope.md` says native CORS "does not
  change the separate EdgionPlugins CORS extension's semantics". That is a scope note, not a
  justification for two engines. No page argues that two engines are required.
- `skills/01-architecture/00-common/05-unification-points.md` lists no CORS unification point, so
  this is an unrecorded duplicate mechanism under design rule D (one canonical implementation per
  cross-cutting mechanism).

## Related findings

- gw-plugin-06, gw-plugin2-04 (same pattern: overlapping plugins with divergent semantics).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs:234-265`: plugin matcher rejects `"null"`, accepts `*`/`**`, a CSV exact cache and a regex.
- `edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs:114-148`: native matcher is a separate Exact/Any/host-wildcard engine with port equality and no `"null"` special case, so the same origin can be judged differently.
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs:82-84`: preflight dispatch special-cases the plugin config.
- `skills/01-architecture/00-common/05-unification-points.md` lists no CORS point, and `skills/04-review/cors/intentional-design/native-cors-rule-scope.md` is a scope note, not a justification for two engines.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `cors/plugin.rs`, `plugin_configs/cors.rs` (`is_origin_allowed` at `:235`), `gateway_api_filters/cors_filter.rs` (`is_origin_allowed` at `:114`) and `preflight_handler.rs` are unchanged; the two engines still exist.
- `plugin_runtime.rs` changed by one line (`:316`, `try_update`). `05-unification-points.md` changed only for Redis/OTLP wording, and `native-cors-rule-scope.md` is unchanged.
