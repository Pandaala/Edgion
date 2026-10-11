import record as r
import subprocess
rows=[
('edgion-tests/src/client/suites/edgion_plugins/key_auth/key_auth.rs','d2256be38233f10140f95a33ee590955089e4133266949e10a32452e7099e1e4','1-140,355-452,1010-1055 actual constants/anonymous bodies/input/registration.'),
('edgion-gateway/src/plugins/http/key_auth/plugin.rs','a0879891ff652cda72d681764a398c7a4338d43dc8b5d30884f7c5ad12555940','Full forwarding/anonymous/metadata/sensitivewrites/stripping.'),
('edgion-gateway/tests/unit/plugins/http/key_auth/plugin_test.rs','830050ba8074e39f98041c159199b2a8010b6c1d36024749121ed58ca4466c92','264-344,364-399 actual complete unit bodies read to/beyond EOF.'),
('edgion-tests/src/server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','881-942 actual /headers lowercase JSON envelope; original router retained.'),
('edgion-tests/integration/conf/EdgionPlugins/KeyAuth/02_EdgionPlugins_default_key-auth-anonymous.yaml','32ab7863567b44532a9293f80453a12423daf119bdd88800ceebb40f15c7ffc6','Full local anonymous policy.'),
('edgion-tests/k8s/conf/EdgionPlugins/KeyAuth/02_EdgionPlugins_default_key-auth-anonymous.yaml','32ab7863567b44532a9293f80453a12423daf119bdd88800ceebb40f15c7ffc6','Full K8s anonymous policy.'),
('edgion-tests/integration/conf/EdgionPlugins/KeyAuth/01_Secret_default_api-keys.yaml','fe4a5efc09bfce7607c4b5090c18d2ee9bd6042a9fd56ab07b9956bc46f8f7b9','Full synthetic Secret Jack mapping.'),
('edgion-tests/k8s/conf/EdgionPlugins/KeyAuth/01_Secret_default_api-keys.yaml','fe4a5efc09bfce7607c4b5090c18d2ee9bd6042a9fd56ab07b9956bc46f8f7b9','Full same synthetic Secret mapping.')]
supports={}
for p,sha,scope in rows:assert r.digest(r.REPO/p)==sha,p;supports[p]=scope
r.confirm('ARCH-377','/root/verify_ip_union','9b1a72f8837a02230c0451c70783a1baf18f651f395b2e7d0d1989361d60986f','23f58e07275c66a9ce9ad6ff2e73392bfe229f3247c9aa2086a07ddcd13ded96',[
'Exact unchanged independent approval: complete empty headers object passes both cases, wrong anonymoususername passes validkey fallback.',
'Exclude affirmative anonymous marker only for existing request without clientidentity headers, not wider production sanitization.',
'Original rules/knowledge/actual routes/resolver/schema/registry/fixtures/unit bodies personally inspected; ARCH348 distinct; no incident/execution/disclosure.'],supports)
s=r.state();s['resume_dispatch']['independent_verifier']='ARCH-373';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
