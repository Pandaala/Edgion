import hashlib,subprocess
import record as r
from compact_receipt import persist
def support(rows):
 d={}
 for p,h,scope in rows:assert r.digest(r.REPO/p)==h,p;d[p]=scope
 return d
a=support([
('edgion-tests/src/client/suites/edgion_plugins/path_normalization/forward_view.rs','cf35de30e2754642ca3d9bd12256d1de51c5d262bafa5ed335f571ae0be0f175','Full echo parsers/absolute controls/six origin assertions/vector.'),
('edgion-tests/src/server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','718-723/3526-3548 real GET echo separate path/query.'),
('edgion-tests/integration/conf/EdgionPlugins/PathNormalization/ForwardView/03_HTTPRoute_default-normalized-forward.yaml','6ba81385080795d432eb3dfeacc21a591fbe2db9c4ff3d91b5ba7ab688c7cb31','Full complete expected path/prefix/backend.'),
('edgion-tests/k8s/conf/EdgionPlugins/PathNormalization/ForwardView/03_HTTPRoute_default-normalized-forward.yaml','c0ca1395dd6bc876e452167b893a30a41eb6202ff75206c238f355ae4eb9cd22','Full K8s complete expected path/prefix/backend.'),
('edgion-tests/integration/conf/EdgionPlugins/PathNormalization/ForwardView/03_HTTPRoute_rewrite-overrides-annotation.yaml','a88b4861e64271aaf8ed44198d30f4778cc7d0122e923fbaa8abc2b2f416f6d3','Full rewrite/raw annotation precedence.'),
('edgion-tests/k8s/conf/EdgionPlugins/PathNormalization/ForwardView/03_HTTPRoute_rewrite-overrides-annotation.yaml','b69d3b792c19c6db49a6004c5e04ab2ee91cfe05d808b02a63f30d6af7ce5a22','Full independent K8s rewrite contract.'),
('edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs','1e78b4bf440134f3f8949ab883bc1f767c0e70055164818c35fc0e2251b25eaf','Finalization caller/1710-1775 latch/tri-state/path/query.'),
('edgion-gateway/tests/unit/routes/http/proxy_http/pg_upstream_peer_test.rs','14ccc89d36290e92ac69aa08b1b152cffb394278085aad72a2693b65dd302399','294-446 real complete URI/query/form/latch/error controls.')])
r.confirm('ARCH-382','/root/verify_ip_union','e615768e3d80377c2fb2c67e908f143f4c8966bd439ed29546cb8c9bd3e08aa0','cefc3cd6f17ee969208d125039389f29f5407ff9d44a824dd9ea5f4d28bc8965',[
'Independent unchanged six origin-form predicates accept appended suffix despite complete-path expectation; exact absolute-form cases limit claim to named assertions.',
'Bounded equality preserves actual read/error/finalization/query/precedence semantics; original rules/normalization pages/all10 fixtures/dispatch personally read. No runtime incident or whole-suite false-PASS claim.'],a)
s=r.state();body=(r.RUN/s['candidates']['ARCH-383']['path']).read_text().split('---',2)[2].replace('Listener selection is the separately tracked ARCH296 cause.','Listener selection is outside this correction.')
assert hashlib.sha256(body.encode()).hexdigest()=='a409097bcb9b09992876c9a553918e706bf9e8f92e5fc8aab8ddfa6c354548bb'
b=support([
('edgion-tests/src/client/suites/edgion_plugins/plugin_condition/all_conditions.rs','1fc6a6d1ea97c5ed9312c727c0260e794d993d5902148f1c2a4dcd1ef410c81f','Full aggregate/failure/sample/fallback/vector assertions.'),
('edgion-tests/integration/conf/EdgionPlugins/PluginCondition/EdgionPlugins_default_condition-all-types.yaml','80dddeaae76ee1c1c9e62250a5f66dfe9e373bfa5a28680a7f97c8d14a9124f6','Full independent nonterminating/100percent/probability/negation entries.'),
('edgion-tests/k8s/conf/EdgionPlugins/PluginCondition/EdgionPlugins_default_condition-all-types.yaml','80dddeaae76ee1c1c9e62250a5f66dfe9e373bfa5a28680a7f97c8d14a9124f6','Full identical current K8s fixture personally read.'),
('edgion-gateway/src/plugins/runtime/conditions/evaluator.rs','9b342831e33c7588d26b279ba776b63aa4374ddabe457cf431fd9cd25715bf4d','Compiled construction/ordinal descriptors/skip/run/detail thresholds/negation/probability.'),
('edgion-gateway/tests/unit/plugins/runtime/conditions/evaluator_test.rs','135be987cf5608923f09142976c42a1d74bef346a9263cc39304740f463b23ff','782-800 actual negated path unit witness.'),
('edgion-gateway/src/plugins/runtime/conditional_filter.rs','2862b351c186c58580300dfee4d4e32371a5f2df9f3328f8be90e9bcc4389ad8','Request wrapper constructor/alias/full invocation/action:cause; body logless.'),
('edgion-gateway/src/plugins/runtime/plugin_runtime.rs','d80096aa53f493497d34dfba80f21f7089bf1ad3086f3b5ab3ff6f1689018c9d','2960-3007 alias-before-invocation retention and response context.'),
('edgion-gateway/src/plugins/runtime/log.rs','bb8bd2221c1bdc9f3e7d54de0db1b93d659e81918ab92534c24363bb4043db28','Fields/serializer/with_alias separate canonical identity.')])
r.confirm('ARCH-383','/root/verify_ip_union','ff75e3ef2bc566a63e575cd69759dca9f38e8a4bb676cf945d3a4cc9aacc70bd','08ae6ed8489b299584366a103b605bf6617b9345ceb26de6087ba3b88851f143',[
'Independent current ordinal descriptors, aggregate unrelated runs and unconditional empty-log negative acceptance confirmed; negate:true /health fixture supplies actual polarity witness.',
'Stable test aliases and one scoped target are feasible existing mechanism; preserve condition/negation/body descriptor semantics and no new request diagnostic payload.',
'Only listener cross-reference revised: current296 approved body is narrower than pending supplemental queue. Both original route fixtures/neighboring alias assertions/Mock/units/topic rules personally read. Static no runtime defect incident.'],b,body)
s=r.state();s['resume_dispatch']['independent_verifier']='ARCH-390';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
