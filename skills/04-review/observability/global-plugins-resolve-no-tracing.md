---
name: global-plugins-resolve-no-tracing
description: Use when reviewing findings that `resolve_global_plugins` / `resolve_route_ext_refs` should warn (tracing) on dangling EdgionPlugins references; explains why the per-request warns were removed and why config-time-reporting / metric replacements were not added.
---

# `global_plugins_resolve` Dangling Refs Are Not Logged (Fixed)

## Conclusion

`resolve_global_plugins` and `resolve_route_ext_refs`
(`edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs`)
**must not** emit `tracing::warn!` (or any `tracing::*`) for dangling
`EdgionPlugins` references. The two warns were removed on 2026-05-24 because:

1. Both helpers are called from `pg_request_filter` once per request to build
   the per-request snapshot `ctx.resolved_global_plugins` /
   `ctx.resolved_route_ext_refs` (the resolved Vec is per-request, not cached
   across requests). A single dangling `global_plugins_ref` or HTTPRoute
   `extensionRef` therefore made the warn fire on *every* request — log
   volume scaling with QPS. This is a textbook violation of
   `skills/03-coding/01-log-safety.md` Iron Rule 3 (zero `tracing` on the
   data-plane hot path).
2. Removing the warns also let the diagnostic-name parameters
   (`gateway_config_name`, `route_name`) be dropped — they existed *only* to
   feed the warn. The resolution result is unchanged: global dangling refs
   are silently dropped from the output, ext-ref dangling slots stay `None`
   (position preserved because `ExtensionRefFilter.index` indexes into the
   Vec). Unit tests `resolve_global_plugins_keeps_order_and_drops_dangling`
   and `resolve_route_ext_refs_preserves_position_with_none_for_dangling`
   cover both.

## Decision Rule

When a review re-reports any of:

1. "`resolve_global_plugins` / `resolve_route_ext_refs` emit `tracing::warn!`
   per request on dangling refs"
2. "dangling-ref reporting should be moved to config-load time (walk refs in
   the `EdgionGatewayConfig` / `EdgionPlugins` ConfHandler, store a
   dangling-set in per-config metadata)"
3. "add a `edgion_*_dangling_refs_total{kind}` counter / PluginLog marker to
   keep per-request visibility"

→ close as **fixed + design intent**. Verify before closing:

- `global_plugins_resolve.rs` contains no `tracing::` calls.
- Neither helper takes a diagnostic-name parameter.

Re-report **(2)** and **(3)** are **not accepted** as part of this fix: a
dangling plugin reference is static config drift, not a per-request event —
the misconfiguration is already observable (the referenced plugin simply does
not run), so the marginal observability does not justify the cross-module
ConfHandler + per-config-metadata change (move 1) or a new always-on metric
(move 2). If a concrete production need for config-time validation surfaces,
promote it to a proper task rather than re-opening this finding.

## Background Notes

- Decision date: 2026-05-24
- Source issue: `tasks/review-2/gateway-routes-04-global-plugin-resolve-tracing-per-request.md` (closed)
- Same class as P3-g4-008 (`cors/force-wildcard-no-tracing-warn.md`): a warn on
  a per-request resolve/preflight helper, removed because the data-plane
  hot-path prohibition outranks the (already config-time-available) signal.
