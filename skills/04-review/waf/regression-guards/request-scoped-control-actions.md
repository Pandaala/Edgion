---
name: request-scoped-control-actions
description: Review the bounded transaction semantics of the selected WAF ctl actions.
knowledge_type: regression-guards
---

# Selected WAF control actions are request scoped

## Conclusion and scope

The strict native WAF compiler admits `forceRequestBodyVariable`,
`requestBodyProcessor`, `ruleRemoveByTag`, and `ruleRemoveTargetByTag` only when
their parsed operands are supported. Other `ctl` actions remain rejected. All
four require `requestBody.inspection: full`. Body controls are legal only in
phase 1. This is a selected SecLang subset, not full CRS compatibility.

## Rationale and evidence

`semantic.rs` classifies unsupported control variants, while `compiler.rs`
converts the selected variants to typed action operands. It resolves target
selectors before publishing the immutable generation and indexes tagged rule
units without modifying them at runtime. `transaction.rs` stages controls with
other matched-chain actions, commits them only for a successful chain, and
applies exclusions to later rule evaluations in the same transaction. The
request-body processor and raw-body visibility are selected before body
publication. A transaction may retain at most 4096 removal and target-exclusion
entries by default; matching work and retained bytes are separately bounded.
Activation charges each tag visit, including duplicate actions and work in
failed chain branches. Target filtering charges each exclusion visit even when
its collection differs from the selected value, then charges selector work
for matching collections. Unsupported XML exclusion paths are rejected during
compilation; the runtime implements only `/*` and `//@*`.
The raw streaming path rejects active controls because it cannot honor the
full transaction semantics.

The Gateway and Controller use the same strict WAF compilation through
`edgion-resources`. Gateway runtime errors map control limits to the WAF
failure policy. The `WafDynamic` control reload checks one request's tag
exemption and a subsequent request's denial. Direct transaction tests cover
the processor override, raw body exposure, tag removal, target exclusion,
chain staging, XML attributes, phase legality, limits, and concurrent requests.

## Verification and re-evaluation

The implementation was checked on 2026-09-24 with targeted WAF transaction
tests and independent review of XML path rejection and control work charging.
Re-evaluate this conclusion if the supported action list, body
publication order, chain staging, target selector semantics, runtime budget,
or Gateway streaming bypass changes. The production acceptance JSON remains a
nine-case baseline; the dynamic control reload extends the Gateway scenario
without changing that frozen corpus.

## Resolution references

- `waf-request-scoped-control-actions`: selected controls added to strict
  compilation and request-local transaction execution on 2026-09-24.
