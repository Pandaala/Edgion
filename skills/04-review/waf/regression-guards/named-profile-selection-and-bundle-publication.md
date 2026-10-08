---
name: named-profile-selection-and-bundle-publication
description: Review WAF profile references, Selector fallback, optional bundle limits, and one-generation publication.
knowledge_type: regression-guards
---

# Named WAF profiles publish one validated generation

## Conclusion and scope

An `EdgionPlugins.spec.waf.policyRef` fixes one typed `WafPolicy` EdgionConfigData. The plugin may select a fixed `activeProfile`, independently reference a Selector with `activeProfileRef`, or use the policy default. The two explicit selection fields are mutually exclusive. The policy names bounded profiles. Each profile
orders `WafRuleBundle` references, optionally pinned by content digest; its required references form
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

The Gateway's source generation retains resolved WAF bundles for revalidation
and later scoped builds. Each `CompiledPlugins` view drops its copied
`resolved_bundles` after runtime construction: class scopes share the cached
WAF executable and do not retain another copy of rule and phrase source text.
The scoped spec still carries the other fields needed for policy, diagnostics,
and admission. This addresses `waf-scoped-source-copy-reduction`; it does not
remove the source generation or change in-flight view ownership.

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
not imply full OWASP CRS compatibility. The explicit repository package
`config/waf/crs-4.25.1-supported-v1` pins a selected 565-unit original core
projection, original setup and separate product tuning; its 62 declared
exclusions remain unsupported by strict admission. Acceptance claims apply
only to that fixed source set and its recorded validation. The actual rule
content, version, profile metadata, and optional self-check digest live in `WafRuleBundle` data;
production acceptance must verify a selected source set before claiming CRS
coverage. Cross-bundle `Include` and phrase-asset lookup remain forbidden, so
optional bundles must be self-contained. Re-evaluate if ConfigData snapshot
semantics, Selector requeue, profile bounds, compiler ordering, publication
retention, or request WAF arbitration changes.

The Gateway WAF request-header view maps the transport-validated H2 Uri
:authority to one borrowed Host view only when the actual HeaderMap lacks Host.
It preserves raw authority bytes (including ports/IPv6), existing Host values
and H1 absence. This projection does not mutate the forwarded header map, parse
another authority or bypass the canonical hostname/Pingora ambiguity guards.
Full-profile wire tests use ordinary authority-only H2 requests.

## Verification

Focused resource, Controller, Gateway, and dynamic integration tests cover the
profile and publication boundaries. The exact commands and outcomes are
recorded in the closing response; the offline acceptance gate covers its
repository-selected rule corpus and does not certify full CRS compatibility.
The Gateway `scoped_waf_views_share_executable_without_retaining_bundle_sources`
test checks two GatewayClass views against one source generation. Existing
Gateway WAF reload tests cover invalid-update retention and in-flight pins;
neither test directly measures process allocator usage.

## Resolution reference

- `tasks/todo/waf-crs-version-selection-and-tuning.md`: implemented as
  operator-owned named rule bundles and Selector-controlled WAF profiles.

## Optional retained aggregate checks

`EdgionGatewayConfig.spec.waf.expensiveChecks` defaults to false in the Gateway
schema and is pinned into each transaction. Direct leaf-engine callers retain
complete original checks through `RuleCheckPolicy::default()` (true); the two
defaults serve different callers.

Strict compilation validates all original rules before classifying optional
families using exact signatures and dependency proofs. The current families
cover HPP units 921170/921180 and multipart aggregate initialization, writes and
checks 922140/922150/922110. Disabling the policy suppresses only proven optional
aggregate effects/checks. Unrecognized signatures or unproven shared observers
retain ordinary execution; rule IDs alone never authorize skipping.

The policy does not disable JSON/XML grammar, required scoped identity and
namespace state, cheap total checks such as 920380, ordinary attack matching,
or necessary capture cleanup. It is not a blanket constant-memory promise.
Controller/Gateway serialization, omitted/false/true configuration, hot updates
and in-flight policy ownership have canonical resource/runtime/live tests.
Re-evaluate the proof in `edgion-waf/src/seclang/expensive_checks.rs` whenever
operators, TX aliases, control flow, or original rule signatures change.

The supported-v1 source manifest accounts for 627 original core units: 59
response-body units, 901320 persistent initialization, and 905100/905110
unsupported control units are excluded. The retained 565 original core units
plus setup and product tuning yield 567 units; the integration fixture adds one
explicit test-owned wire guard, yielding 568. Response-body inspection remains
outside current scope; unsupported original units still fail strict admission.
