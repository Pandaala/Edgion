from compact_receipt import persist
import record as r
s=r.state();p=r.RUN/s['candidates']['ARCH-383']['path'];old=p.read_text();(r.RUN/'WITHDRAWN-ARCH383-PROPOSAL-20261011-G.md').write_text('Withdrawal: original analyst corrected the descriptor witness before independent review. This is historical proposal evidence, not an active finding.\n\n'+old)
body='''
# Correlate AllConditions assertions with the plugin entry being tested

`all_conditions.rs` tests individual condition families using totals over every `Mock` entry in the selected resource. Positive cases generally require only `count_mock_plugins_ran > 0`; several negative cases return PASS even when their expected skip descriptor is absent and the skipped count is zero. The deterministic-probability case compares whether any Mock ran across five requests. These assertions do not establish the outcome of the condition named by each case.

Both current fixture trees attach many independent non-terminating Mock entries to the same request. The 100% probability entry supplies an unrelated run on every ordinary request. Removing the cookie-controlled entry or making its run gate fail would still satisfy `key_exist_cookie_run`. An incorrectly varying deterministic entry also remains invisible: every sample's aggregate Boolean stays true because another entry ran. A successfully parsed access log containing no Mock entries can satisfy unconditional negative fallbacks such as `key_match_regex_skip`.

The committed exclusion fixtures provide a current witness. Their path predicate is placed under `skip` with `negate: true`. For `/health`, matching returns true before negation and false afterward, so the path entry runs. The actual evaluator and negated-path unit test confirm this. `test_exclude_path_skip` nevertheless passes: its legacy `skip:keyMatch` substring does not match current ordinal-bearing descriptors, and its fallback returns PASS with an aggregate skipped count regardless of that count. This is static source evidence; no suite execution or production predicate defect is claimed.

Give the existing fixture entries stable aliases and select the target request-stage plugin log within the expected EdgionPlugins resource. Require exactly one matching target, then assert its own run or skip outcome. For deterministic sampling, compare that target across the existing five same-key requests; for 0% and 100%, require its specified outcome on every sample. Missing targets and missing expected descriptors must fail instead of falling back to counts. Match the current canonical ordinal-bearing descriptor contract. Neighboring `conditions.rs` already uses aliases, and production dispatch creates `PluginLog::with_alias` before executing or skipping each entry.

Exact response-header markers are an alternative for positive execution, but absence alone cannot prove a skipped entry existed. The unused header-log helper is unsuitable: current non-terminating Mock logs say only `mock headers set`. Add no request diagnostic payloads to identify fixtures.

Preserve case identities, request vectors, transport/log-read failures, deadlines and canonical log ownership. Do not change condition, negation, short-circuit, body-phase or descriptor-threshold semantics. Stronger assertions expose the exclusion-fixture polarity mismatch; resolve its intended configuration explicitly rather than weakening evaluation. Listener selection is the separately tracked ARCH296 cause. No generic registry, runtime framework or performance claim is proposed.
'''
sections=old.split('---',2);p.write_text('---'+sections[1]+'---'+body);s['candidates']['ARCH-383'].update(sha256=r.digest(p),analyst_correction='Original proposal withdrawn; exact analyst replacement corrects current ordinal-bearing descriptor/fallback witness before independent verification.',withdrawn_proposal='WITHDRAWN-ARCH383-PROPOSAL-20261011-G.md');r.save(s)
persist('PART-504','/root/analyze_outbound',{
'all_conditions.rs':('1fc6a6d1ea97c5ed9312c727c0260e794d993d5902148f1c2a4dcd1ef410c81f','Full1-1408 five disjoint complete ranges','ARCH383 target condition outcome not established by aggregate/unconditional assertions; numeric31180 separately pending296.'),
'conditions.rs':('24313f636fea45b88befaa0426663abaea517e34e255e06956ee699504e89e11','Full1-354','Three aliascorrelated Cors cases fail missing/read/parseconditions and retain descriptor suppression/composite evidence. Accepted.'),
'mod.rs':('9093e010544d8a866b55cbdba87fc5db15393a41448ca972c685a6f9a3ec8add','Full1-13','Actual parent/child exportregistry accepted.')},[
('02-gateway/05-plugin-system.md','4cfcad0d1d1d43bfe530ed69f28d79c804c1f8b17e1b4017df1f5a0642aee299','241-375 actualcurrent ConditionSet/skiprun/negate/ordinal/bodyphase originalcontracts.'),
('runtime/conditional_filter.rs','2862b351c186c58580300dfee4d4e32371a5f2df9f3328f8be90e9bcc4389ad8','1-105 actual requestalias/skip path.'),
('conditions/evaluator.rs','9b342831e33c7588d26b279ba776b63aa4374ddabe457cf431fd9cd25715bf4d','1-190,195-295,335-391,395-679,751-782,884-978 compiled dispatch/ordinalcauses/skipfirst/runsecond/negation/probability.'),
('conditions/evaluator_test.rs','135be987cf5608923f09142976c42a1d74bef346a9263cc39304740f463b23ff','430-533,733-816 actual boundary/probability/negatedpath units.'),
('runtime/plugin_runtime.rs','d80096aa53f493497d34dfba80f21f7089bf1ad3086f3b5ab3ff6f1689018c9d','2950-3046 actual aliasedrequestlog dispatch/retained outcomes.'),
('http/mock/plugin.rs','1f9b16ad20fe0b014d3e7ec905506d577e8d55a2053c3c7b59110ff466e3f71b','Full1-86 nonterminatingfixedlog/responsemarker.'),
('PluginCondition/EdgionPlugins_default_condition-all-types.yaml','80dddeaae76ee1c1c9e62250a5f66dfe9e373bfa5a28680a7f97c8d14a9124f6','Full1-377 bothtopologies independent100percent andexclusionpolarity.')],
'Both eight fixtureYAMLs full/actualregistry2560-2583/suitekey1551-1564/framework75-156,207-228,532-600/pluginmetadata+singleowner+descriptor originals/mandatoryintegration personally read. Personaloriginal rulesfull/freshhash/current/no nested. No SSE/bodyphase case or streamcoverage claim. Current compiled ordinal contract corrects initial proposed unrelateddescriptor witness; unconditionalfallback stillproves target falsePASS. No runtimepredicate defect/execution/mutation/supportcredit.',candidate_ids=['ARCH-383'])
s=r.state();q=s.setdefault('pending_supplements',{}).setdefault('ARCH-296',[])
if 'REPORT-PART-504.json' not in q:q.append('REPORT-PART-504.json')
s['resume_dispatch']['analysis_outbound']=['PART-508','PART-509','PART-510'];r.save(s)
