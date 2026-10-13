---
name: review-plugin-runtime
description: Review knowledge for plugin-runtime, grouped by decision type.
---

# plugin-runtime review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [AI session capability](intentional-design/ai-session-capability.md)

- [Selected-WAF session capability](intentional-design/selected-waf-session-capability.md)

- [EdgionPlugins last-known-good is scoped by logical resource key](intentional-design/edgionplugins-key-scoped-last-known-good.md)
- [plugin_runtime `add_from_*` Builder Duplication — Extract-Helper — DESIGN-TRADEOFF](intentional-design/add-from-builders-duplication.md)
- [No Per-Route Override or Exclusion of Global Plugins (Envoy typed_per_filter_config) — DESIGN-TRADEOFF (accepted)](intentional-design/no-per-route-global-plugin-override-or-exclude.md)
- [plugin_runtime.rs Central Match Dispatch — Audited Design Tradeoff](intentional-design/plugin-runtime-god-match.md)
- [StreamPluginStore key normalization depends on the attachment owner](intentional-design/stream-plugin-store-key-normalization.md)

## Compatibility constraints

- [Response plugin iteration and fork async capability boundaries](compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md)

## Regression guards

- [ProxyRewrite path-token boundaries](regression-guards/proxy-rewrite-path-token-boundaries.md)

- [Response header mutation policy and body-owner framing](regression-guards/response-header-mutation-policy.md)

- [ExtensionRef tokenized plugin-log contract](regression-guards/extension-ref-tokenized-plugin-log-contract.md)

- [Request-template policies, bounds, and Secret provenance](regression-guards/request-template-policy-unification.md)

- [Borrowed plugin header access](regression-guards/borrowed-plugin-header-access.md)

- [Header modifier validation and construction](regression-guards/header-modifier-validation.md)

- [Gateway API request-transform validation and preparation](regression-guards/gateway-api-request-transform-validation-and-preparation.md)

- [GatewayClass-scoped policy and runtime publication](regression-guards/scoped-policy-runtime-publication.md)

- [AiGuard ordering errors fail closed at publication](regression-guards/ai-guard-ordering-publication.md)

- [AiGuard span bounds and message-skip semantics](regression-guards/ai-guard-span-bounds-and-skip-semantics.md)

- [Dye stage and producer parity](regression-guards/dye-stage-producer-parity.md)

- [Invalid stream GeoIP generation denial and recovery](regression-guards/invalid-stream-geoip-generation-fail-closed.md)
- [EdgionConfigData partial publication is incremental and atomic](regression-guards/edgion-config-data-incremental-atomic-publication.md)
- [Internal HTTPRoute Edgion-auth Construction Failures Must Seal Fail-Closed](regression-guards/gw-plugin-runtime-01-httproute-external-auth-invalid-plugins-never-sealed.md)
- [Stream plugin key normalization](regression-guards/stream-plugin-key-normalization.md)

- [Scoped response-body filter dispatch](regression-guards/scoped-response-body-filter-dispatch.md)

## Superseded

- [Historical per-reference HTTP generation boundary](superseded/request-plugin-reference-generation-boundary.md)
