---
name: waf-support-inspection-must-not-imply-publication
description: Review bounded WAF compatibility inspection and strict publication decisions.
knowledge_type: regression-guards
---

# WAF support inspection must not imply publication

## Conclusion and scope

`WafConfig::inspect_support()` returns a value-free, bounded report for local
tools and other consumers. `analysis_complete` means SecLang reached semantic
compatibility analysis. Only `strict_validation_passed` means the exact
inline `WafConfig.rules` and `request_body` settings passed the strict compiler and
request-body inspection check. A configuration with `bundleRefs` reports an
unresolved-reference failure in local inspection, because the CLI has no
Controller-resolved bundle contents. The Controller and Gateway continue to use
`WafConfig::compile_generation()` for publication.
The strict compiler now admits `@detectSQLi` and `@detectXSS`, but an inspection
result still does not establish Gateway request behavior.

## Rationale and evidence

The SecLang frontend can enumerate unavailable capabilities before strict
validation. A compatibility inventory alone can therefore exist when
publication fails, including a missing phrase asset or a rule needing complete
upstream response-body aggregation. The report maps typed compiler failures to
fixed categories and emits numeric locations, never raw source, operands,
regexes, asset handles, or file identities. `edgion-cli waf inspect` consumes
the public resource-layer report without authentication or a network call.
Its document input is capped at 16 MiB; the report retains at most 128 findings
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
bundle reference handling, and report truncation. CLI tests cover local command parsing and value-free
text presentation. Re-evaluate if the compiler, diagnostic categories,
report bounds, resource schema, or publication path changes.

## Resolution references

- `waf-rule-support-diagnostics`: implemented as a bounded local inspection
  report with a reusable resource-layer API.
- `waf-rule-bundles-and-assets`: typed `EdgionConfigData` bundles are resolved
  before strict compilation; local inspection reports unresolved references.
