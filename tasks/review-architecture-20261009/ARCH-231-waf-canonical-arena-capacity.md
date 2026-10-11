---
id: ARCH-231
status: candidate
kind: behavior-defect
priority: P2
partition: PART-162
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# Account canonical WAF arena capacity before publishing the generation

`GroupedMatcherState::compile` in `edgion-waf/src/seclang/compiler.rs:3106–3123` reserves the published target and transformation arenas using their original occurrence counts. `target_bytes` reserves all literal/path selector bytes; `target_records` reserves all active target records; `target_plans` and `transformation_plans` reserve all active members; and `canonical_transformations` reserves the full original effective-transformation vector length. Canonicalization then inserts only unique plans and values. `copy_target_plan` at 1965–2025 and `insert_transformation_plan` at 2132–2174 charge only bytes/records actually inserted. Successful deduplication leaves the requested spare capacities allocated, but those capacities have no corresponding retained compiler charge.

The vectors are moved through `GroupedMatcherState` and `CompiledGeneration::publish_with_work` into the immutable published generation (`3523–3540`, `3900–3925`). They are never compacted or replaced by exact unique-sized owners. Consequently, `compiler_owned_bytes`, `retained_bytes` and the generation limit checks are based on unique lengths for these arenas while the generation retains occurrence-sized capacity. This is a discrepancy in explicitly requested Vec backing capacity, independent of allocator over-allocation or fragmentation.

A concrete supported source shape is:

```text
SecRule REQUEST_HEADERS:x "@eq 1" "id:1,phase:1,pass,t:lowercase"
SecRule REQUEST_HEADERS:x "@eq 2" "id:2,phase:1,pass,t:lowercase"
```

Both rules remain independently scheduled with different IDs and integer predicates. They share one canonical target plan, one literal selector byte, one lowercase transformation value and one transformation plan. Nevertheless, the compiler reserves capacity for two records, two target plans, two selector bytes, two transformation values and two transformation plans. Each vector retains at least the requested capacity, whereas its canonical insertion charges only one unique item. With N such valid rules, this per-arena difference scales with N−1 without requiring regex compilation, optional capture engines, excluded rules or a request body. Unit/node/action/source limits still bound the input; this is not an unbounded allocation or an incident claim.

The producer is live on both sides of the architecture. `WafConfig::compile_resolved_candidate_detailed` (`edgion-resources/.../plugin_configs/waf.rs:1056–1230`) compiles validated operator bundles through `compile_rule_bundles_with_external_budget` or strict two-pass selection. The include frontend invokes `compile_validated`/`compile_validated_with_budget` (`include.rs:1018–1060`, `1257–1298`). `PluginValidation for WafConfig` uses that compilation for admission, and Gateway `WafPlugin::try_new` (`plugins/http/waf/mod.rs:107–163`) independently compiles the configuration and retains the resulting generation in an Arc. The private inline-source test seam is not necessary to reach the reservation behavior.

The existing budget model treats requested capacity as owned storage, separately from logical occurrences and process memory. `json_inspection_plan.rs:132–145` charges growing arena capacity, allocator-provided extra capacity and reallocation overlap to the same `MatcherCompileWork` generation/compilation limits. `RegexProgramInterner::new` in the primary and `capture_hit_plan.rs:73–97` likewise charge their reserved capacities. The original fused JSON guard requires compiler capacities and overlap to be bounded before publication, and final optional attachments to use the remaining global/per-program budgets. Its explicit qualification that compiler forecasts exclude fragmentation and are not observed process memory does not explain away an omitted, deliberately requested base arena reservation. No retrieved original decision authorizes this canonical-length exception.

The ownership problem is local: logical canonical insertion and backing-store lifetime use the same counter without a reservation owner. Deduplication should reduce published logical inventory; it must not silently turn capacity outside the canonical budget into apparent headroom for later required or optional compilation.

Make these five arenas use the existing compiler budget owner for allocation admission and retained capacity. A bounded solution can retain the occurrence-sized preallocation, charge its requested capacity before each fallible reservation, reconcile allocator-provided extra capacity, and remove the insertion-based double charge. If preserving useful memory savings from canonicalization matters, use bounded fallible growth for unique output entries under the same capacity and overlap accounting instead. Keep this private to the compiler's canonical arena construction; no new public allocation framework, request-path state, registry or observability family is needed.

An alternative is an additional bounded pass that computes exact canonical output sizes before reserving them. That requires carrying canonical identities through another pass and charging its work and coexistence; it is a more invasive choice than making the current reservation owner accurate. Shrinking after construction alone does not admit the allocation or account for its construction overlap.

Preserve exact source and profile identity, collision equality barriers, first-assigned canonical IDs, source/member order, logical count limits, cumulative hash/probe/comparison work, strict original validation before tag pruning, requirement discharge, phase schedules and markers, capture semantics, JSON/XML domain separation, body-mode admission and independent Controller/Gateway compilation. Keep errors content-free and publication all-or-nothing. Correct capacity admission may reject a candidate earlier under a tight existing byte limit; preserve deterministic failure ordering within the chosen private allocation path and never compensate by raising production limits.

The inspected tests prove canonical lengths and shared IDs (`compiler_test.rs:1456–1491`), source-order schedules (`1737–1832`), exact reported generation/peak thresholds (`1922–1960`), optional fallback (`2054–2119`) and strict selection (`2277–2331`). They do not independently compare these five published arena capacities with their retained charges. Future private-access tests should cover repeated and distinct header target plans, literal/path bytes, empty and nonempty transformations, chains and static selection; inspect capacity versus charge at grouping and publication, and test exact/one-less admission without using the same reported total as the sole oracle. Existing semantic/order/body/error tests remain required.

This is one base compiler arena-capacity cause. It is distinct from the semantic frontend's temporary transformations-before-final-t:none issue, request-owned scratch growth and optional matcher optimizations. Existing architecture findings and the pending WAF optimization design were searched; that task concerns transaction/search performance and retains canonical budget requirements. No throughput, RSS, default-limit exhaustion or externally exploitable memory claim is made. The review is static; no compilation or test was executed.
