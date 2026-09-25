---
name: waf-support-inspection-must-not-imply-publication
description: Review bounded WAF compatibility inspection and strict publication decisions.
knowledge_type: regression-guards
---

# WAF support inspection must not imply publication

## Conclusion and scope

`WafConfig::inspect_inline_source()` returns a value-free, bounded report for a
local SecLang source. `analysis_complete` means SecLang reached semantic
compatibility analysis. Only `strict_validation_passed` means that source and
its default request-body inspection settings passed strict compilation. This
local source check does not resolve the referenced WafPolicy, its selected profile, or any digest-pinned
`WafRuleBundle` reference. The Controller and Gateway use
`WafConfig::compile_generation()` on the selected resolved profile for publication.
The strict compiler now admits `@detectSQLi` and `@detectXSS`, but an inspection
result still does not establish Gateway request behavior.

## Rationale and evidence

The SecLang frontend can enumerate unavailable capabilities before strict
validation. A compatibility inventory alone can therefore exist when
publication fails, including a missing phrase asset or a rule needing complete
upstream response-body aggregation. The report maps typed compiler failures to
fixed categories and emits numeric locations, never raw source, operands,
regexes, asset handles, or file identities. `edgion-cli waf inspect` consumes a
raw SecLang source file and the public resource-layer report without
authentication or a network call. Its source input is capped at 16 MiB; the report retains at most 128 findings
and 128 rule summaries and exposes omitted counts.

## Alternatives and limits

This is a local inspection surface, not a Controller status or Gateway admin
API. A failed parse or semantic analysis has `analysis_complete: false` and no
compatibility counts. Missing rule IDs are rejected during semantic analysis;
the report gives a safe physical line and does not invent an ID. Consumers such
as EdgionCenter may use the public API but must apply their own input limits
and access control. An inspection result is not evidence of Gateway wire
behavior; the separate selected-profile acceptance gate owns that evidence.

## Verification and re-evaluation

Resource-layer tests cover mixed supported and unavailable rules, strict
rejection, missing IDs, malformed syntax, missing phrase assets, redaction,
bundle reference handling, and report truncation. CLI tests cover raw source
input and value-free text presentation. Re-evaluate if the compiler, diagnostic categories,
report bounds, resource schema, or publication path changes.

## Resolution references

- `waf-rule-support-diagnostics`: implemented as a bounded local inspection
  report with a reusable resource-layer API.
- `waf-rule-bundles-and-assets`: typed `EdgionConfigData` bundles are resolved
  before strict publication compilation; local source inspection does not bind them.
