---
name: no-per-route-global-plugin-override-or-exclude
description: Use when reviewing findings that Edgion has no per-route override/disable of global plugins (no Envoy typed_per_filter_config; a route cannot exclude or reconfigure a listener-level global plugin; no disable_global_plugins escape hatch for /healthz-style routes); covers why a route-side global-plugin override/exclude API is a wide-impact feature, not a narrow review fix, and what workaround already exists.
status: accepted-tradeoff
finding_id: filter-03
closed: 2026-06-14
knowledge_type: intentional-design
---

# No Per-Route Override or Exclusion of Global Plugins (Envoy typed_per_filter_config) — DESIGN-TRADEOFF (accepted)

## Conclusion

Edgion's global plugins (`EdgionGatewayConfig.spec.global_plugins_ref`,
`edgion-resources/src/resources/edgion_gateway_config.rs:81`) are resolved once per publication into
the class scope's shared `PluginScope::resolved_globals` slice, installed per request into
`ctx.resolved_global_plugins`, and run unconditionally before all route-level plugins
(`edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:436-447`, plus the four response
phases). There is no field on `HTTPRoute` / `GRPCRoute` / any route rule that lets a route
override a specific global plugin's config or disable it for that route only — Edgion has no
equivalent of Envoy's VirtualHost/Route/WeightedCluster `typed_per_filter_config`.

Fix suggestions of the form "add a `globalPluginsOverride` / `excludePlugins` /
`disable_global_plugins` field on `HTTPRouteRule`", "build a per-route effective-global-plugin
list at preparse", or "mirror Envoy's per-route filter disabling" are **not accepted** as
review-issue fixes — this is a new cross-layer public-API feature that belongs in `tasks/todo/`
via the brainstorming workflow, and a route-side workaround already exists.

## Core Rationale

**1. This is a wide-impact public-API feature, not a narrow bug**

A per-route override/exclude API requires coordinated changes across three layers: a new schema
field on `HTTPRouteRule` / `GRPCRoute` (`edgion-resources`, a public CRD surface), controller-side
validation of the override/exclude entries on resource ingest, and a gateway preparse step that
merges the global list with the rule's override/exclude entries into an effective per-route list.
That is exactly the wide-impact profile (public API + shared schema + multiple coordinated layers)
the review-issue scope gate is meant to push into `tasks/todo/` for brainstorming, not a minimal
low-risk fix. It also carries real design surface — override-vs-exclude precedence, ordering of
the merged list, interaction with the existing global-plugin ordering, and per-route validation
semantics — that needs a design pass, not a drive-by patch.

**2. A route-side workaround already exists**

Global plugins already support `conditions` / `skip` matching (header/path/etc.) on the plugin
itself, so an operator can exempt routes today (e.g. skip a global `RateLimitLocal` on `/healthz`).
The gap is ergonomic, not a hard blocker: the exemption is encoded on the global plugin instead of
on the route, which couples the plugin's config to knowledge of exempt routes. That is an
operational inconvenience that grows with route count, not a correctness defect or a security
hole — consistent with a medium-severity feature request.

**3. The hot path is already shaped for the feature, which confirms it is additive**

`ctx.resolved_global_plugins` is installed per request from a prepared per-class-scope slice
(`PluginScope::resolved_globals`), so the eventual feature would need a preparse merge step plus
a per-route prepared effective list installed in its place, not new per-request resolution work. That the runtime is "ready" does not make
this a small fix — the cost and risk live in the schema, controller validation, and preparse
merge, all of which are still cross-layer additive work that should be designed deliberately.

## Fix Suggestions Not Accepted

- Add a `globalPluginsOverride` / `excludePlugins` / `overridePlugins` field on `HTTPRouteRule`
  (or `GRPCRoute`) as a review-issue fix — new public CRD schema + controller validation + gateway
  preparse merge; promote to `tasks/todo/` and design via brainstorming.
- Add a `disable_global_plugins: true` route-level escape hatch — same wide-impact surface; it is
  the degenerate case of the override/exclude feature, not a separate small fix.
- "Just" build the effective per-route global-plugin list at preparse — the preparse merge is the
  easy part; it is meaningless without the schema field and controller validation that gate it.

## Re-evaluation Triggers

Re-open this decision only if:

- The per-route override/exclude feature is prioritized — then it is promoted to `tasks/todo/` and
  designed via brainstorming (schema + controller validation + preparse merge), not closed here.
- A concrete operator scenario shows the global-plugin `conditions` / `skip` workaround is
  insufficient (e.g. an override that cannot be expressed as a condition at all, not merely an
  inconvenient one).
- Gateway API standardizes a per-route filter override/disable mechanism that Edgion must conform
  to.

## Reference Cases

- Finding `filter-03-no-per-route-global-plugin-override` (`tasks/review-envoy/`), 2026-06-14 —
  ⚠️ accepted tradeoff; closed without code change.
- Same review-set family of accepted architecture tradeoffs:
  [filter-iteration-control-sync-phase-tradeoff.md](../compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md),
  [body-filter-read-only-by-design.md](../../http-body/intentional-design/body-filter-read-only-by-design.md),
  [listener-port-set-frozen-at-startup.md](../../listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md).
- Source: `edgion-resources/src/resources/edgion_gateway_config.rs:81` (`global_plugins_ref`);
  `edgion-gateway/src/plugins/publication/views.rs` (`PluginScope::resolve_http_runtimes`, prepared
  `resolved_globals`);
  `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:436-447` (unconditional global
  plugin execution before route plugins).
