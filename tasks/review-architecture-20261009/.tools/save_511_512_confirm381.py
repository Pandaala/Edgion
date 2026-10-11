import hashlib,subprocess
import record as r
from compact_receipt import persist
s=r.state();body=(r.RUN/s['candidates']['ARCH-381']['path']).read_text().split('---',2)[2].replace('openid_connect.rs:277','openid_connect.rs:283')
assert hashlib.sha256(body.encode()).hexdigest()=='e911521ee2eee4b383b628017e89641f20880746c2c0f9dab6acb703e971635f'
rows=[
('edgion-tests/src/client/suites/edgion_plugins/openid_connect/openid_connect.rs','a96d69cbca354098acdce6ffde01de4e89bd97bca60403dc92968aadd2a4517f','Full suite; RSA fixture structure personally inspected without private material output.'),
('edgion-tests/src/server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','4434-4454 actual matching RS256 JWK.'),
('edgion-gateway/src/plugins/http/openid_connect/token_verify.rs','9602a082d738a190f5df20b3d861d28aa4642d237d95d9cd32f5a02136a842b8','1-218 real decode and typed format-only Auto fallback.'),
('edgion-gateway/src/plugins/http/openid_connect/error.rs','1ffb56f6ae20563e36425c8dc51e29615099c6ee8a0945e1cf47d47da470e810','199-316 static redacted messages and distinct signature/format mappings.'),
('edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test.rs','b898bea6c62fcd39ec46bf0efe62eb2805a013eb7b03a9f44b46ebc5a9a4b8c7','1735-1765 actual Auto fallback discrimination.'),
('edgion-tests/integration/conf/EdgionPlugins/OpenidConnect/03_EdgionPlugins_default_openid-connect-jwks.yaml','1178f083984e8740c732b00bce2c198199d3e324e3f9c546c698935d4bc3560e','Full JwksOnly RS256 fixture.'),
('edgion-tests/k8s/conf/EdgionPlugins/OpenidConnect/03_EdgionPlugins_default_openid-connect-jwks.yaml','a30363d55de3a218b0fc4f26f07b06c68bb5b7049e389e36c1d66ae59216be4d','Full independent in-cluster discovery fixture.'),
('skills/04-review/oidc/regression-guards/authentication-response-byte-caps.md','3eb35121ae9ce5a21c4524e819ec49ef3d3c919e5c465716ce31606e724d511b','Full original bounded readers, fallback and prior fixture branch correction.')]
supports={}
for p,h,scope in rows:assert r.digest(r.REPO/p)==h,p;supports[p]=scope
r.confirm('ARCH-381','/root/verify_ip_union','af2ccfa415de9a62f9b03621ef60feb3ace6998b345d9e68ad49394dc54713df','0910d6f5b8be98d252b4361e8ed96c296bc8dd33e922ec84187000439e36af57',[
'Independent arithmetic and repository-owned key/JWK structure establish noncanonical final sextet; no dependency execution or pinned decoder claim.',
'Decoded-byte mutation/re-encoding is bounded and preserves header/claims/401, Auto policy, topology and redaction.',
'Only opening location revised from277 to283; exact body and canonical claims independently approved. Original rules and actual callers/tests personally read.'],supports,body)
persist('PART-511','/root/analyze_conversion',{
'dynamic.rs':('27390a4df5dce3096a4bdf36eaf287849e118b894e21416a4e68e2a16bcbf198','Full1-116','Initial/rotated/deleted key and fallback checks strict; literal listener uses extend pending296 only; restoration is present.'),
'mod.rs':('17c6087f8708e8331f85963dade50ede0cda8579916dd6dc68d6dd478683676b','Full1-10','Exports and factories accepted.'),
'request_access_policy.rs':('7d7b47a4bbe5420c3fcf94b77f1ca35ca717463e3aa755944c253a39e45631e0','Full1-760','All15 static cases and actual dependency/decision/HMAC/digest contracts reviewed. Canonical Controller status projection; authority port input intentional.')},[
('request_access_policy/plugin.rs','589cab7cea5550b0cd8926449ee0ededb2cff93c2f15dd34e26a6e01bf48c608','340-483 constructor/profile/matching/terminal decisions.'),
('request_access_policy/plugin_test.rs','b60d48ed287bf85c80fcbb06a9e521eb79a5dd19431a36f9065535f799e8de07','469-592 sibling fallback and capture/digest contracts.'),
('scenarios/request_access_policy_hmac_dynamic.rs','7a532588dcb205e47039c8426709e67f3e5359fc86887fdd492f9bb1de165408','Full1-68 initial/rotate/delete/restore.'),
('suites/status/helpers.rs','0236de331bc053f9ba00610543ef4e7a8932a9d49e129110f068091d98f823dc','102-151/214-241 and retained1-74 canonical current Controller envelope.'),
('RequestAccessPolicy/06_EdgionPlugins_edgion-test_gac-hmac.yaml','cf7d78aaa1d8193f4dd0f06b9f89cd600084ceced801456494bd6c02b73ef065','Full1-65 authored match/digest fallback; local read.'),
('RequestAccessPolicy/DynamicTest/initial/01_EdgionPlugins_gac-hmac-dynamic.yaml','ecdd25925a340cfa6526c4af44c9987410d15acacf8fad0778194ed618014a14','Full1-30 K8s non-loopback fallback intentional.'),
('header-hmac-condition-verifier-prebuilt.md','d2e87acc573d14d06e451702c498332f2e3d588a6d8a2a72eee230d99cb7649f','Full original compiled header/body async owner.'),
('13-edgion-plugins.md','138713f63892574a7949257bdf58a2f2147148ed88790f379bfa05d948eb12ff','1-115 canonical resolution/status/publication.')],
'Original root/index/graded/routing/topic rules personally current; 18 local YAMLs and three differing K8s fixtures fully read, remaining K8s compared byte-identical only. Static dispatch2760-2778/dynamic1708-1734/TSV41 and MatchCore read. No execution, writes or support coverage; no new candidate.')
s=r.state();s['pending_supplements']['ARCH-296'].append('REPORT-PART-511.json');r.save(s)
persist('PART-512','/root/analyze_conversion',{
'mod.rs':('4afb67dd3dd2703cf9aef72384032b1e597543efa8b465367328d1e39a81b886','Full1-3','Explicit suite export accepted.'),
'request_id.rs':('78ce8449c58dc78f88f3e3e0357fc4dc2c38410cf2d9366c27f9ac47263c61b7','Full1-210','All nine W3C/duplicate/default/source/flags/suppression/early404 identity cases strictly observe wire and canonical logs. No new cause.')},[
('plugins/http/request_id/mod.rs','e67cbd2491f797626dc4aea24c121dc040a4b70d79c687f97173489abbb4181b','Full1-44 canonical compiled incremental plugin.'),
('request_trace_id/w3c.rs','54c0848f5c48389f152b8948eb0a8f65baefa49ea2133d09040f48365ddd9f6d','Full1-110 strict parser/root.'),
('request_trace_id/mod.rs','003101f5874221609d131dac0369f4c715fc8f76402666c863bee553ae24097c','110-144/255-284/310-324 canonical once initialization and reconciliation.'),
('lib_test/request_id_tests.rs','7c985cc4272f7bce744d8a8aa67455c2ae5c535d088ec11b0351597e465416c0','Full1-158 actual Pingora composition/policy/visibility units.'),
('request-trace-id-lifecycle.md','c680a28cc8b36352a4f7025041872a9c5150c7698ceec1800c34f0295f8846de','Full1-157 original canonical lifecycle and scoped inheritance.'),
('RequestId/04_EdgionPlugins_multiple.yaml','01bd1f04214cf6b197f5c98f0cca29d3d88019ecf8198bfae09135e238e374ca','Full1-20 local sources/flags; K8s bytes compared only.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','881-936 actual header JSON producer and retained current route591.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','2730-2745 selected suite; retained3915-3955 iteration.')],
'Original rules/topics personally read and freshly revalidated. All8 local YAMLs fully read/all8 K8s bytes compared identical only. Dynamic fixtures outside invocation excluded. Scoped authored00 test predicate creates no second production parser. Static only; no execution/mutations/support primary credit.')
s=r.state();s['resume_dispatch']['analysis_outbound']=['PART-514','PART-515','PART-516'];s['resume_dispatch']['independent_verifier']='ARCH-383';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
