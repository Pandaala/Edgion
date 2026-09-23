---
name: review-resources
description: Review knowledge for resources, grouped by decision type.
---

# resources review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [deny_unknown_fields on LinkSys credential structs vs forward compat — ACCEPTED-TRADEOFF (the attribute is the inline-credential defense)](intentional-design/deny-unknown-fields-credential-structs-forward-compat-tradeoff.md)
- [`edgion.io/managed-by` Is Inert on EdgionConfigData — WONT-FIX (no ownership mechanism)](intentional-design/edgion-config-data-managed-by-is-inert.md)
- [Gateway process configuration is local YAML only](intentional-design/gateway-process-config-is-yaml-only.md)
- [GEP-2257 Duration Field Scope — CONVERTED SET IS CLOSED](intentional-design/gep2257-duration-field-scope.md)
- [`PartiallyInvalid` is not route-exclusive — DOCS-DRIFT (fixed)](intentional-design/partially-invalid-not-route-exclusive.md)
- [ReferenceGrant validation is a controller-global policy — DESIGN-DECISION](intentional-design/reference-grant-validation-controller-scoped.md)
- [Schema validation placement and Kubernetes availability without bundle locking](intentional-design/schema-validator-non-k8s-asymmetry.md)
- [Unknown EdgionPlugin variant rejects the EdgionPlugins resource — ACCEPTED-TRADEOFF (strict typed enum is intentional)](intentional-design/unknown-plugin-variant-strict-enum.md)
- [Validation errors are untyped `Vec<String>` — ACCEPTED-TRADEOFF (the machine-readable taxonomy already exists as the Gateway API `.status.conditions.reason` field)](intentional-design/validation-errors-untyped-string-vs-status-reason-taxonomy.md)

## Compatibility constraints

- [`serde_ignored` is blind through flatten AND adjacently-tagged enums — walk the schema instead](compatibility-constraints/serde-ignored-blind-spots-schema-walk-instead.md)

## Regression guards

- [Declared CRD objects use Kubernetes structural schema](regression-guards/declared-object-crd-structural-schema.md)
- [Duration schema admission patterns](regression-guards/duration-schema-admission-patterns.md)
- [EdgionBackend unknown-field handling is mode-specific](regression-guards/edgion-backend-unknown-fields-are-mode-specific.md)
- [EdgionGatewayConfig process-global values aggregated deterministically — FIXED](regression-guards/edgion-gateway-config-global-aggregation-deterministic.md)
- [EdgionPlugins closed enums drifted from Rust — FIXED (reconciled + guarded)](regression-guards/edgion-plugins-crd-type-enum-drift.md)
- [Terminal local replies require a final HTTP status](regression-guards/request-local-reply-final-status-validation.md)
