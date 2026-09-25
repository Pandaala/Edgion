---
name: named-profile-selection-and-bundle-publication
description: Review WAF profile references, Selector fallback, optional bundle limits, and one-generation publication.
knowledge_type: regression-guards
---

# Named WAF profiles publish one validated generation

## Conclusion and scope

An `EdgionPlugins.spec.waf.policyRef` fixes one typed `WafPolicy` EdgionConfigData. The plugin may independently reference a Selector with `activeProfileRef`. The policy names bounded profiles. Each profile
orders digest-pinned `WafRuleBundle` references; its required references form
an atomic base and may aggregate multiple ConfigData objects. The Controller
captures the Selector and every referenced bundle in one ConfigData read,
strictly compiles candidate profiles, and publishes only the chosen profile's
accepted sources to the Gateway. Two plugins can share one policy and select
different profiles through their own Selectors. A Selector update requeues the referring
EdgionPlugins resource and changes the compiled Gateway generation through
normal conf_sync. In-flight requests retain the generation captured at entry.

`defaultProfile` must name a valid profile. An absent, disabled, or unknown
Selector choice uses that validated default. A failed required default profile
invalidates the candidate; the existing key-scoped last-known-good policy then
retains a prior activatable generation when allowed, or seals the first invalid
generation. The Controller's candidate identity is not proof of what the
Gateway currently runs if last-known-good retention applies.

An explicitly optional bundle is an independent partial-good unit. The
Controller adds optional references in declaration order only when the
aggregate still strictly compiles. A missing, tampered, or incompatible
optional reference is skipped and surfaced through `PartiallyInvalid`. Required
references never use this skip path. A bundle's internal rule errors cannot be
silently skipped; the bundle is the indivisible unit. Optional source failure
can reduce protection, so authors must use `optional: true` only for additions
whose omission is acceptable. The default profile always contains a required
base.

## Rationale and evidence

`WafPolicyData` owns the operator profile and bundle reference shape; `WafConfig` owns the fixed policy and per-plugin Selector references plus Gateway-side strict revalidation. The ConfigData handler registers policy-to-bundle dependencies; the plugin handler registers its policy and Selector references, resolves one captured source set, and emits
bounded status diagnostics without rule text. `WafPlugin` still holds one
immutable generation and owner token across request and response phases; the
Selector is not read again per phase. This preserves the single-transaction
request boundary and avoids compilation on the request path.

The Controller rejects an over-limit policy before copying its child sources, then
captures one policy snapshot and retains only accepted reference indices for inactive profiles.
After choosing the active profile, it materializes that profile's bundle sources
from the captured snapshot. Retaining owned source copies for every usable
profile multiplies memory by the profile count when profiles share large bundles.

The captured Selector target is checked against the current `ReferenceGrant`
using the consuming EdgionPlugins as the source; bundle grants use the WafPolicy as their source.
before its `active` name is used. Status derives Selector availability from
that same capture rather than reading the store a second time. These checks
prevent a target change between an earlier authorization probe and snapshot
capture from activating an unauthorized profile or reporting a different
generation than the one published.

Request-wide WAF arbitration prefers Route, then Gateway, then Global. The
configured `priority` chooses among candidates within the same scope; equal
priority keeps declaration order. This is separate from profile selection
within one WAF resource. Neither mechanism merges two active WAF generations
for a request.

## Limits and re-evaluation

Profile names such as `low`, `mid`, and `high` are operator-defined. They do
not imply that Edgion ships or validates an OWASP CRS release. The actual rule
content, version, profile metadata, and digest live in `WafRuleBundle` data;
production acceptance must verify a selected source set before claiming CRS
coverage. Cross-bundle `Include` and phrase-asset lookup remain forbidden, so
optional bundles must be self-contained. Re-evaluate if ConfigData snapshot
semantics, Selector requeue, profile bounds, compiler ordering, publication
retention, or request WAF arbitration changes.

## Verification

Focused resource, Controller, Gateway, and dynamic integration tests cover the
profile and publication boundaries. The exact commands and outcomes are
recorded in the closing response; the offline acceptance gate covers its
repository-selected rule corpus and does not certify full CRS compatibility.

## Resolution reference

- `tasks/todo/waf-crs-version-selection-and-tuning.md`: implemented as
  operator-owned named rule bundles and Selector-controlled WAF profiles.
