Withdrawal: original analyst corrected the descriptor witness before independent review. This is historical proposal evidence, not an active finding.

---
id: ARCH-383
status: candidate
kind: behavior-defect
priority: P3
partition: PART-504
analyst: /root/analyze_outbound
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Correlate AllConditions assertions with the plugin entry being tested

`all_conditions.rs` tests individual condition families using totals over every `Mock` entry in the selected resource. Positive cases generally require only `count_mock_plugins_ran > 0`; several negative cases return PASS even when their expected skip descriptor is absent and the skipped count is zero. The deterministic-probability case compares whether any Mock ran across five requests. These assertions do not establish the outcome of the condition named by each case.

Both current fixture trees attach many independent non-terminating Mock entries to the same request. The 100% probability entry supplies an unrelated run on every ordinary request. Removing the cookie-controlled entry or making its run gate fail would still satisfy `key_exist_cookie_run`. An incorrectly varying deterministic entry also remains invisible: every sample's aggregate Boolean stays true because another entry ran. A successfully parsed access log containing no Mock entries can satisfy unconditional negative fallbacks such as `key_match_regex_skip`.

The committed exclusion fixtures provide a stronger current witness. Their path predicate is placed under `skip` with `negate: true`. For `/health`, canonical matching returns true before negation, false afterward, so the path entry runs. The method exclusion entry, also under negated `skip`, matches the complement of OPTIONS and skips this GET. `test_exclude_path_skip` accepts any Mock containing `skip:keyMatch`, so that unrelated method entry can make the path case pass while its own entry ran. The actual evaluator and its negated-path unit test confirm these predicate semantics. This is static source evidence; no suite execution or production predicate defect is claimed.

Give the existing fixture entries stable aliases and select the target request-stage plugin log within the expected EdgionPlugins resource. Require exactly one matching target, then assert its own run or skip outcome. For deterministic sampling, compare that target's outcome across the existing five same-key requests; for 0% and 100%, require its specified outcome on every sample. Missing target records and missing expected descriptors must fail instead of falling back to aggregate counts. The neighboring `conditions.rs` already demonstrates alias-based selection, and production dispatch creates `PluginLog::with_alias` before executing or skipping each entry.

Exact response-header markers are an alternative for positive execution checks, but absence alone cannot prove that a skipped entry existed. The unused header-log helper is unsuitable: current non-terminating Mock logs say only `mock headers set`. Do not add per-request diagnostic payloads to identify fixtures.

Preserve all existing case identities, request vectors, transport/log-read failure handling, wrapper deadlines and canonical log ownership. Do not change shared condition, negation, short-circuit, body-phase or descriptor-threshold semantics. Stronger assertions may expose the existing exclusion-fixture polarity mismatch; resolve its intended configuration explicitly rather than weakening canonical evaluation. Listener selection is the separately tracked ARCH296 cause. No generic test registry, runtime framework or performance claim is proposed.
