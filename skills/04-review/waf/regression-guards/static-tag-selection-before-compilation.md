---
name: static-tag-selection-before-compilation
description: Review trusted YAML WAF rule selection, strict baseline validation, compiler bounds and local inventory claims.
knowledge_type: regression-guards
---

# Static WAF tags select whole units before compilation

## Conclusion and scope

`WafPolicy.profiles.<name>.ruleSelection` is a canonical, opaque typed tree in
`edgion-waf`. It is configuration-time metadata selection, distinct from the
shared request `Condition` domain. Missing include/exclude means true/false;
exclusion wins. Selection never reactivates source-removed units. Tags are exact
case-sensitive literals from validated members; IDs and phases belong to roots.
Chains are indivisible and structural markers remain. The existing frontend
forbids authored tags on chain children; selection does not expand that syntax.

The leaf expands one immutable source set, validates and compiles the entire
baseline, and applies the real plugin body-mode admission before pruning. When
membership changes, it drops that generation, reparses the same original input,
validates the original graph again, removes activity, rebuilds reachable target
regex requirements and compiles selected plans. Thus excluded invalid regex,
assets, unsupported syntax or body-mode dependencies remain invalid. No-op
selection reuses its baseline. Fully omitted policies retain the original path;
an omitted profile in a mixed policy uses single-pass external-budget compilation.

Canonical work is cumulative across inventory and both compiler passes. Compiler
peak accounting includes concurrently retained source/provenance, typed trees,
prepared programs, adapter views, masks, target scratch and bounded report scratch.
Frontend transients retain their independent limits; this is not a global loading
heap or process peak guarantee. Published and in-flight generations may coexist.

## Resolution and publication

The Controller prepares all bounded profile selections once before capturing
child sources. Its private invocation owner is bound to the complete profile-name
and selection-tree set, then reused by required and optional borrowed trials.
A failed optional aggregate is skipped as before, even when selection would
exclude its malformed rule. Only the final chosen result contributes numeric
Accepted diagnostics. Unmatched atoms and valid empty selections do not add
resolution warnings or imply PartiallyInvalid. Original rule bytes, pins and
selection trees travel through conf_sync; the Gateway independently recompiles.
No evaluator, catalog or new cache is attached to the request path.

## Inspection and decoding boundaries

Direct JSON/YAML selection decoding rejects observable duplicates, null, wrong
scalar types, mixed/unknown keys and oversized trees before typed descent.
Tag decoding uses deserialize_any: YAML deserialize_str would coerce numeric,
boolean and null scalars. The existing ConfigData normalized DOM can collapse
raw duplicates before typed decoding; direct-decoder guarantees do not apply to
keys already erased by that boundary.

Local `waf tags` validates typed bundle envelopes and bounded serialized inputs.
Tag queries identify semantic inventory; selected/excluded queries distinguish
semantic plans from compiled membership. Failed/incomplete inventory uses unknown
counts, not misleading zeros. Full inventory can coexist with failed compilation.
Body admission is deliberately unchecked for local bundles. Reports limit rows,
ID references, escaped metadata/output and pointers without altering selection.
No rule operands, phrase contents or internal Include paths enter reports.

## Evidence and re-evaluation

The canonical tests are leaf tag_selection/compiler tests, resource waf_policy,
waf_tags and WafConfig tests, Controller optional/publication tests, Gateway store
old/new ownership tests and CLI tests. The pinned language omission/score corpus
is `edgion-resources/tests/waf_tag_selection.rs`; the existing shared WafDynamic
scenario has a policy-only selection phase and restoration. The historical
`tasks/pending/waf-tag-selection/05-verification.md` record is absent from the
current tree. Run the canonical tests and shared scenario using the
[testing guide](../../../05-testing/01-integration-testing.md) for fresh gates
and environmental limitations; fixture presence is not live acceptance. Re-evaluate on frontend/compiler bounds, metadata syntax, policy
transport, trial ownership, source graph or output-contract changes.
