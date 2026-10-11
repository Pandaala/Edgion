---
id: ARCH-383
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-504
analyst: /root/analyze_outbound
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---
# Correlate AllConditions assertions with the plugin entry being tested

`all_conditions.rs` tests individual condition families using totals over every `Mock` entry in the selected resource. Positive cases generally require only `count_mock_plugins_ran > 0`; several negative cases return PASS even when their expected skip descriptor is absent and the skipped count is zero. The deterministic-probability case compares whether any Mock ran across five requests. These assertions do not establish the outcome of the condition named by each case.

Both current fixture trees attach many independent non-terminating Mock entries to the same request. The 100% probability entry supplies an unrelated run on every ordinary request. Removing the cookie-controlled entry or making its run gate fail would still satisfy `key_exist_cookie_run`. An incorrectly varying deterministic entry also remains invisible: every sample's aggregate Boolean stays true because another entry ran. A successfully parsed access log containing no Mock entries can satisfy unconditional negative fallbacks such as `key_match_regex_skip`.

The committed exclusion fixtures provide a current witness. Their path predicate is placed under `skip` with `negate: true`. For `/health`, matching returns true before negation and false afterward, so the path entry runs. The actual evaluator and negated-path unit test confirm this. `test_exclude_path_skip` nevertheless passes: its legacy `skip:keyMatch` substring does not match current ordinal-bearing descriptors, and its fallback returns PASS with an aggregate skipped count regardless of that count. This is static source evidence; no suite execution or production predicate defect is claimed.

Give the existing fixture entries stable aliases and select the target request-stage plugin log within the expected EdgionPlugins resource. Require exactly one matching target, then assert its own run or skip outcome. For deterministic sampling, compare that target across the existing five same-key requests; for 0% and 100%, require its specified outcome on every sample. Missing targets and missing expected descriptors must fail instead of falling back to counts. Match the current canonical ordinal-bearing descriptor contract. Neighboring `conditions.rs` already uses aliases, and production dispatch creates `PluginLog::with_alias` before executing or skipping each entry.

Exact response-header markers are an alternative for positive execution, but absence alone cannot prove a skipped entry existed. The unused header-log helper is unsuitable: current non-terminating Mock logs say only `mock headers set`. Add no request diagnostic payloads to identify fixtures.

Preserve case identities, request vectors, transport/log-read failures, deadlines and canonical log ownership. Do not change condition, negation, short-circuit, body-phase or descriptor-threshold semantics. Stronger assertions expose the exclusion-fixture polarity mismatch; resolve its intended configuration explicitly rather than weakening evaluation. Listener selection is outside this correction. No generic registry, runtime framework or performance claim is proposed.
