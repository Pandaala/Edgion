---
name: review-plugin-runtime
description: Review knowledge for plugin-runtime, grouped by decision type.
---

# plugin-runtime review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [HTTP request plugin reference generation boundary](intentional-design/request-plugin-reference-generation-boundary.md)

- [plugin_runtime `add_from_*` Builder Duplication — Extract-Helper — DESIGN-TRADEOFF](intentional-design/add-from-builders-duplication.md)
- [No Per-Route Override or Exclusion of Global Plugins (Envoy typed_per_filter_config) — DESIGN-TRADEOFF (accepted)](intentional-design/no-per-route-global-plugin-override-or-exclude.md)
- [plugin_runtime.rs Central Match Dispatch — Audited Design Tradeoff](intentional-design/plugin-runtime-god-match.md)
- [StreamPluginStore key normalization depends on the attachment owner](intentional-design/stream-plugin-store-key-normalization.md)

## Compatibility constraints

- [Plugin Chain Has No Envoy-Style Iteration Control (StopIteration / async pause-resume) — DESIGN-TRADEOFF (accepted)](compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md)

## Regression guards

- [Invalid stream GeoIP generation denial and recovery](regression-guards/invalid-stream-geoip-generation-fail-closed.md)
- [EdgionConfigData partial publication is incremental and atomic](regression-guards/edgion-config-data-incremental-atomic-publication.md)
- [HTTPRoute Native ExternalAuth Construction Failures Must Seal Fail-Closed — FIXED](regression-guards/gw-plugin-runtime-01-httproute-external-auth-invalid-plugins-never-sealed.md)
- [Stream plugin key normalization](regression-guards/stream-plugin-key-normalization.md)
