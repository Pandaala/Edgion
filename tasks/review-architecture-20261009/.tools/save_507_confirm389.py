from compact_receipt import persist
import record as r
import subprocess
persist('PART-507','/root/analyze_conversion',{
'dynamic.rs':('86a71a15d7beac32d4ed7dfb57586d3ce0d0e3e0e34ad7b82245b505a6a250fb','Full1-89','Initial200/update429 missingkeycases canonicalURL/failtransport avoid CMS timing; existing264localrestore onlypending.'),
'mod.rs':('322653e48d046c5cb0db3eabc7d13100254df9e3ce4194837ca9c38cabac5643','Full1-7','Explicit static/twophase exports accepted.'),
'rate_limit_local.rs':('81901044189f637f37a739ea65e08453debfbf1aae858ba83dfe971a7847b361','Full1-807 four complete chunks','Ninecases exactquota/sharedroute/multikey/multirule/miss/retryafter/canonicallog/emptybucket, headers_present explicitlydisabled. Nine literalURLs acrosseight static cases same296pending; tracelog622/dynamic canonicalURL.')},[
('rate_limit_local/plugin.rs','4abb3cb88124cca228f0229e0e17ca8bf2197bdef85da88a6544d8173dd32491','Full1-296 immutableconfig/OnceLockCMS/preparedreject/sharedkey/missquota distinctowner.'),
('scenarios/rate_limit_local_dynamic.rs','31351016f658ed8b8dcd75fee8ccdea52f6335b448fc4e9f13c285786beb1826','Full1-44 actualfactory/run noinitialapply/restore.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','2690-2721,1708-1734,3683-3763 retained3915-3955 real staticphase/injectedcontract/directscenario owner.'),
('run_k8s_dynamic_scenarios.sh','95b8aa354ed3f308004e989daef9896718ba40bc4fa843a4b94e4f08094c7394','478-546,846-902,927-984 actualinitialreset/boundedobserver/dirtycleanup/prepost owner, notfull.'),
('RateLimitLocal/DynamicTest/initial/EdgionPlugins_rate-limit-dynamic.yaml','e8f3d371a25c28bc80120a144f42efa99eba083e0b70c894c94e0a1a365e7b2d','Full1-21 local Allow initial.'),
('RateLimitLocal/DynamicTest/updates/EdgionPlugins_rate-limit-dynamic.yaml','3e628550aa8f583b0a0bbbc1b3355770e549d6b99ae0590d64241a98ba2d3199','Full1-21 local Deny update sameidentity.'),
('05-testing/01-integration-testing.md','e260042b508fa0956873d1b3c059531f1733a7c15423c548081afb71703a10e1','235-263,310-337,407-441 meaningfulassertions/ports/scenario-ownedinitial+cleanup, retained originalbroaderread notfreshfull.'),
('key-extraction-shared-helper.md','2810f46e1cfab85087bd67e6a588b9a5f8198ed0fc9ead2148362d80b038b7f3','Full1-67 accepted sharedkey/callerrejectmapping.')],
'Same264localdirect lifecycle only: scenario30-36 initialprobe beforeapply updatesAllow->Deny no success/failure restoration; secondlocaldirectrun fails initial. Current K8s wrapper explicitlyresets before/after+EXIT, doNOTgeneralize witness there. Same296staticURLs at38,80,167,257,322,405,441,525,706 currentlyfixturesagree31180; ownershipportabilitynotpresentmismatch. Full sevenstaticYAMLseach andboththreefiledynamic/README/currentrealunits1-135/key58-114/factory1615-1664/ScenarioCtx1-147/K8srow26/headeremitterintentional113 recovered. Personalfullrules/progressiveoriginals/freshunchanged/no nested, no test/CMSempirical/performance/execution/mutation/supportcredit.')
rows=[
('edgion-tests/src/client/suites/edgion_plugins/rate_limit_redis/rate_limit_redis.rs','bd00ec15de5cb126b44f2f7bd8ebb037b17e2a546a44e6f6c4b666f893e3d7a6','Full ledger/localinclude/reorderedapply/convergence/exhaustedbucket/restore/registry.'),
('edgion-tests/integration/conf/EdgionPlugins/RateLimitRedis/06_EdgionPlugins_multi-quota.yaml','7809f093a9656c451a74891941386cf22a31f73c4a43231135e40b09026e988f','Full twodocs localdefaultRedis/pluginidentity/Deny/namedquota/route.'),
('edgion-tests/k8s/conf/EdgionPlugins/RateLimitRedis/06_EdgionPlugins_multi-quota.yaml','80feea14f15f15f6f83ffdca863d234ede9a15b93f9f90e7f0ecf39d00167b0a','Full twodocs currentK8s edgion-defaultRedis/plugin/route.'),
('edgion-tests/k8s/conf/EdgionPlugins/Dsl/00_LinkSys_default_redis-test.yaml','231cf080247ecfb8ffe13e4a045bc4cd3a1894ce19dfea62a21c80a5396b1a84','Full actualnamespace/ref/incluster/Secret/connectionowner.'),
('edgion-gateway/src/link_sys/runtime/store.rs','b995c16f0e1b9cbde6cdc9c7a74576464b9f0184b9316e4924aac97c230970de','1-96,580-625 canonicalidentity/exactRUNTIMESlookup.'),
('edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs','8300ea1ea1c66a0c513021699ae66e92ad5008d44485e923f401c07db360827c','228-311,370-460 fixedreject/Deny/runtimeclient/dependencyfailure.'),
('edgion-tests/k8s/scripts/run_k8s_integration.sh','9a858cd81d47c74f20b375a503b10f1c52107ab413116783c8a49c4ec16e42e1','Defaultroot;2070-2098,2556-2569,2670-2750,2879-2898 selection/batch/publication.'),
('edgion-tests/k8s/scripts/common/run_test_client_batch.sh','00089d28fdfa37374020345714e51f3e84776aec27412945f1646117d2a4ec72','1-250 actualmodeexport/staticinvocation no caseexclusion.')]
supports={}
for p,sha,scope in rows:assert r.digest(r.REPO/p)==sha,p;supports[p]=scope
r.confirm('ARCH-389','/root/verify_ip_union','9d728f47eb7d864462edb12818beb6b8acd4999110bdebc236b1e8a48e23a0c2','0f5c1344891bad5cbc2dc4e2cc55ba0cb46eeecfb0f2d6bed08c6c65494d05e4',[
'Independent unchanged approval bothlocal-source publications replaceK8s Redisidentity; YAMLapplyunchanged/runtimeexactkey/noalias.',
'ActualK8s declarations edgion-default+edgion-test butnodefault; custompreseedmaskqualified; correctselectedsource preservesquotaorder/counter/Deny.',
'Originalrules/namedquota/LinkSys/failurepolicy/currentbothfixtures/strictpublisher/actualinvocation/realunits personallyread; 264344distinct; no execution.'],supports)
s=r.state()
for i in ['ARCH-264','ARCH-296']:
 q=s.setdefault('pending_supplements',{}).setdefault(i,[])
 if 'REPORT-PART-507.json' not in q:q.append('REPORT-PART-507.json')
s['resume_dispatch']['analysis_conversion']=['PART-511','PART-512','PART-513'];s['resume_dispatch']['independent_verifier']='ARCH-381';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
