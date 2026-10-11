---
id: ARCH-073
status: candidate
kind: behavior-defect
priority: P3
partition: PART-213
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Clamp per-plugin estimator capacity before an overflowing unit conversion

RateLimitLocalConfig::validate_and_compile in edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_local.rs578 computes estimator_slots_k * SLOTS_K using unchecked usize multiplication, then applies the existing minimum and configured maximum. SLOTS_K is 1024 in the canonical rate_limit_local_config module. check_fields validates rate, scope, status, header names, keys and interval, but does not restrict estimatorSlotsK; the Option<usize> field is serde-visible under the preserved plugin configuration and expressly documents capping at the global maximum.

A source-level witness is estimatorSlotsK = usize::MAX / 1024 + 1, a representable value on the build target. Its mathematical slot count exceeds usize::MAX. Under checked-overflow compilation the multiplication panics before the cap; under unchecked wrapping arithmetic it becomes zero, and ordinary valid default global bounds select the 1024-slot minimum instead of the configured maximum. The workspace production profile inherits release and supplies no overflow-check override. These are arithmetic/source facts, not an executed panic, Controller-status observation or measured collision/throughput result.

Shared PluginValidation returns check_fields only, so otherwise-valid active entries pass the source admission used by Controller and Gateway. Gateway PluginRuntime's normal RateLimitLocal arm constructs through create_with_outcome_slot; RateLimitLocal::create_with_outcome_slot clones the configuration and calls validate. This multiplication therefore occurs during cold runtime construction, rather than only in a detached test helper. Later get_rate reads the resulting effective_slots and passes it to Rate::new_with_estimator_config. The finding asserts a build-profile-dependent failure or wrong prepared capacity, not an authentication bypass, proven process termination or unconstrained heap allocation.

The intended contract is already clear: values above the maximum are accepted and capped, and zero is raised to the minimum. Existing test_estimator_slots_capped_at_max accepts 2048K with an explicit 256K maximum; test_estimator_slots_minimum accepts zero. Rejecting every oversized operator value would change that contract. The avoidable mistake is evaluating an unbounded intermediate before enforcing its bounded result.

Use saturating multiplication for this per-plugin K-to-slots conversion before the unchanged minimum/maximum chain, or equivalently compare against a carefully overflow-safe threshold before multiplying. Saturating multiplication is the smaller correction: every mathematical overflow must exceed a representable max_slots and should resolve to that maximum. Keep the current valid-global-policy assumption and existing diagnostic/cache order. Do not introduce another policy source, hot-path validation, request locks or arbitrary operator ceiling.

Preserve default inheritance, zero/minimum behavior, exact nonoverflowing values, current oversized-value warning boundary, stateless source validation, serde-hidden prepared fields, interval preparation and unrelated skew clamping. Preserve Instance/Cluster approximate thresholds, dynamic instance count, Continue/Reject and same-resource Redis overflow outcomes, canonical key composition/missing-key policy, failure policies, response header ownership and redaction. Local and Redis emitters retain their deliberate differences. This report does not redesign or prove validity of independently configured global default/max capacities; the global module also has multiplications and deserves separate explicit policy analysis before broadening the correction. It must not be silently changed as incidental cleanup.

The full current local configuration tests and validation-trait tests were read. They cover normal/default/zero/2048K capacities, global max selection, structural errors, interval and Cluster/skew behavior, but contain no usize-boundary capacity witness. Original local-allowance-redis-overflow, response-header-name-validation, canonical key extraction/composition, header-injection-duplication and Redis-fail-open decisions were read; none authorizes overflow-before-clamping. Existing review candidates and tasks were searched for estimator_slots_k/get_effective_slots/SLOTS_K and yielded no owning finding.

Future verification should use explicit valid global counts to avoid global mutable test state, exercise zero, normal, above-cap, usize::MAX / SLOTS_K + 1 and usize::MAX inputs, and assert the same capped effective count without panic in checked arithmetic. Preserve invalid-field precedence and repeat normal Gateway construction wiring with the real configuration owner. Do not use runtime performance tests to establish simple arithmetic semantics or widen production visibility for tests.

Benefit: one cold bounded arithmetic owner respects its published cap across profiles. Scope: per-plugin RateLimitLocalConfig unit conversion only, with global policy normalization explicitly excluded. Priority P3 because the witness requires extreme operator input; no incident or quantitative performance claim. Static read-only review; no builds, tests, runtime probes or implementation. Primary and caller/original/test hashes are recorded with PART-213; independent verification is pending.
