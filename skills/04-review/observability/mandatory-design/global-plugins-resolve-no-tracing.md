---
name: global-plugins-resolve-no-tracing
description: Use when reviewing findings that global/Gateway plugin-list preparation (`PluginScope::resolve_http_runtimes`) or `resolve_route_ext_refs` should warn (tracing) on dangling EdgionPlugins references; explains why the per-request warns were removed and why config-time-reporting / metric replacements were not added.
knowledge_type: mandatory-design
---

# Dangling Plugin References Are Not Logged During Resolution (Fixed)

## Conclusion

Neither plugin-reference resolution path emits `tracing::warn!` (or any
`tracing::*`) for a dangling `EdgionPlugins` reference:

- **Global and Gateway-level lists** (EGC `globalPluginsRef`, Gateway
  `edgion.io/edgion-plugins`) are resolved once per publication by
  `PluginScope::resolve_http_runtimes`
  (`edgion-gateway/src/plugins/publication/views.rs`) into the prepared
  `PluginScope::resolved_globals` and `GatewayScope::resolved_http` slices.
  The request path (`pg_early_request_filter` / `pg_request_filter`) only
  clones those slices into `ctx`. Dangling keys are dropped from the slice.
- **Route ExtensionRefs** are still resolved per request by
  `resolve_route_ext_refs`
  (`edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs`);
  dangling slots stay `None` because `ExtensionRefFilter.index` indexes the
  resolved list positionally.

History: the per-request warns were removed on 2026-05-24 because the global
and route lists were both resolved on every request at that time (Gateway-level
plugin keys were added later and followed the same per-request path), so a
single dangling `global_plugins_ref` or `extensionRef` made a warn fire per
request — log volume scaling with QPS, a violation of
`skills/03-coding/01-log-safety.md` Iron Rule 3 (zero `tracing` on the data-plane
hot path). Removing the warns also dropped the diagnostic-name parameters that
existed only to feed them. On 2026-11-06 (early-01) the global/Gateway lists
moved to publication time; the no-logging decision was kept (decision rules
below), and the resolution result is unchanged.

Coverage: `prepared_global_slice_matches_reference_order_and_tracks_publication`,
`prepared_gateway_slice_matches_keys_and_reuses_unchanged_owner` and
`prepared_gateway_slice_keeps_ambiguous_placeholder`
(`edgion-gateway/tests/unit/plugins/publication/mod_test.rs`), and
`resolve_route_ext_refs_preserves_position_with_none_for_dangling`
(`edgion-gateway/tests/unit/routes/http/proxy_http/global_plugins_resolve_test.rs`).

## Decision Rule

When a review re-reports any of:

1. "`resolve_route_ext_refs` (or the request-path install of global/Gateway
   slices) emits `tracing::warn!` per request on dangling refs"
2. "gateway-side dangling-ref reporting should be added at config-load time" — including
   "`PluginScope::resolve_http_runtimes` now runs at publication, so it can
   warn" — or "walk refs in the `EdgionGatewayConfig` / `EdgionPlugins`
   ConfHandler, store a dangling-set in per-config metadata"
3. "add a `edgion_*_dangling_refs_total{kind}` counter / PluginLog marker to
   keep per-request visibility"

→ close as **fixed + design intent**. Verify before closing:

- `global_plugins_resolve.rs` and `PluginScope::resolve_http_runtimes` contain
  no `tracing::` calls.
- No resolution helper takes a diagnostic-name parameter.

Re-reports **(2)** and **(3)** are **not accepted** here: a dangling plugin
reference is static config drift, not a per-request event, and the original
decision judged the marginal gateway-side observability (a config-time warn or
an always-on metric) not worth the cross-module change. If a
concrete production need for gateway-side config-time diagnostics surfaces,
promote it to a proper task rather than re-opening this finding. Controller-side
status reporting of unresolved references is out of scope for this page (see
`tasks/improve-issues/ctrl-proc2-02-egc-status-hides-unresolved-references.md`).

This page covers logging only. Whether a missing global/Gateway target should
fail closed instead of being dropped is a separate open finding:
`tasks/improve-issues/gw-core-02-missing-gateway-and-global-plugins-fail-open.md`.

## Background Notes

- Decision date: 2026-05-24; publication-time move: 2026-11-06.
- Source issues: `tasks/review-2/gateway-routes-04-global-plugin-resolve-tracing-per-request.md` (closed);
  `early-01` (precompute global and Gateway plugin runtime slices, closed).
- Same class as P3-g4-008 (`cors/force-wildcard-no-tracing-warn.md`): a warn on
  a per-request resolve/preflight helper, removed because the data-plane
  hot-path prohibition outranks that per-request signal.

## Resolution references

- `gateway-routes-04` — 2026-05-24.
- `early-01` — 2026-11-06 (global/Gateway lists prepared at publication).
