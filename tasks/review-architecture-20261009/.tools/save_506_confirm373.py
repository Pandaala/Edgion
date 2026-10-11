from compact_receipt import persist
import record as r
import subprocess
s=r.state();p=r.RUN/s['candidates']['ARCH-386']['path'];old=p.read_text();assert old.count(':313-389')==1;p.write_text(old.replace(':313-389',':313-391'));s['candidates']['ARCH-386']['sha256']=r.digest(p);s['candidates']['ARCH-386']['analyst_correction']='Exact full owning function range corrected by original analyst before independent verification.';r.save(s)
persist('PART-506','/root/analyze_conversion',{
'proxy_rewrite.rs':('8191854e8af42cb0843bd53c796402dbf86590069b4b53046b59bacd9e717720','Full1-678 three complete ranges','Nine real rewrite/encoding/capture cases accepted; existingcombined test disconnected ARCH386. Defaulted responsecannotpasspositivecontains. ContextURLs canonical.'),
'mod.rs':('feecea01124d289d6894ffbddc6e5739c9d044522c082fc93373186af1ae247d','Full1-9','Actual export accepted.')},[
('ProxyRewrite/05_EdgionPlugins_combined.yaml','aaaf503c147aeb84e72ab2665268d4aa23848c35680a8cb45b6c698658ca7ecd','Full1-26 both topology actualcombined plugin.'),
('ProxyRewrite/HTTPRoute_default_proxy-rewrite.yaml','0a1671f5299e19b691e32df8bcddd34e870f61ec222471b541871045e2589701','Full1-140 K8s route combinedprefix.'),
('proxy_rewrite/plugin.rs','c257049692250ee377fe5d1dd3e981d77fb5f0d5d08a48f2a61a9d66858086a9','280-369,713-751 actualHost/addsetremove order.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','734-767,810-834 both real echohandlers/casings.'),
('client/framework.rs','dd7b89a260befd5283e63c7a405e878918949ad17a618af0b23138312fed0383','608-825 current casesvector-only actualiteration.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','2680-2689 andretained3915-3955 real invocation.'),
('proxy_rewrite/plugin_test.rs','690f1b25cb41d89b5eb4ae77031acab490199f2c4b80fc82e070616f2ba057b1','366-446 actualEOF,encoding units recovered400-427 gap.')],
'Full nineYAML each topology+README/separateResponseRewrite153-171/adapterHost1430-1457/originalappendheaders15/testing/context/template inspected. README historical coverage/pathprose stale only. Dedup037072 no productionencoding/preparation claim, no296348. Original rules fullpersonalcurrent/gates; sourceonly/noexecution/mutation/supportcredit.',candidate_ids=['ARCH-386'])
rows=[
('edgion-tests/src/client/suites/edgion_plugins/ldap_auth_tls/ldap_auth_tls.rs','3729698af350e6bc98c9e8e81fe01ceb7a3d724b70e52fa989e28811a057f9fa','Full fourcases and mTLS prerequisite/predicates/registry.'),
('edgion-tests/k8s/conf/EdgionPlugins/LdapAuthTls/11_Deployment_edgion-test_openldap-tls.yaml','1ad82fa9dbc880149fb3867431d0eae5a10c8bd1ecf8f6cbc2b30201a4cf5556','Full single tryserver/cert mounts/seed/readiness.'),
('edgion-tests/k8s/conf/EdgionPlugins/LdapAuthTls/12_Service_edgion-test_openldap-tls.yaml','4f9576697fc3a3d4fbd8c4f62ba08dbf45035defa098ecdb22c57b007e02b5c0','Full bothports same server636.'),
('edgion-tests/integration/conf/Services/ldap-openldap-tls/docker-compose.yml','7e90ff478f6c62825b1784104c4a00afe6c029168ddeec1b88aeb2d7feca7e61','Full distinct try/demand fixture servers/certs/mappings/health.'),
('edgion-tests/k8s/scripts/run_k8s_integration.sh','9a858cd81d47c74f20b375a503b10f1c52107ab413116783c8a49c4ec16e42e1','1-35,130-181,1185-1280,1418-1460,2070-2128,2574-2584,2832-2865,2880-2926 strictapply/prep/readiness/cleanup/reuse/registry.'),
('edgion-gateway/src/plugins/http/ldap_auth/plugin.rs','d4a7d9f9d41bd609de65f0a3fe2d470f24e375b42450e0b19bf3d6504c02e206','190-280 canonicalTLS/trust/clientauth vs no-clientauth construction.'),
('edgion-gateway/tests/unit/plugins/http/ldap_auth/plugin_test.rs','2f34688a68c69dbc594934409d7d38e32346df53b6ceea6086f80df91319680c','806-955 actual materialowner/encryptedtransport tests notlivehandshake.'),
('edgion-tests/integration/scripts/integration/ldap_egress/scenario.py','91debb2a0bfc3688f448e2be94f2e6d7c911c5dc1552b110b1e8af42fefeac49','259-297,367-391,477-513 existing standalone try/demand and with/withoutidentity matrix.')]
supports={}
for p,sha,scope in rows:assert r.digest(r.REPO/p)==sha,p;supports[p]=scope
r.confirm('ARCH-373','/root/verify_ip_union','5d8297c93c08d11b8b6963f7c9fcfc97f6163c2f6b65433a1e95a1eb94aa2577','983c28e00a48b48921a7e62c647227475808b84d5d1c809ab4ef8a21fc66d0e4',[
'Exact independent unchanged approval: K8s single declared trylistener sharedCA-only/mTLSports doesnotestablish requiredclientidentity prerequisite.',
'Existing local separate demand and standalone with/withoutidentity matrix acknowledge scopedcoverage; preserveCAonly/STARTTLS/wrongCA.',
'Original rules/LDAPknowledge/currentfixtures/readiness/cleanup/TLSunitbindings personallyread; no vendorimage/livehandshake or executedincident claim.'],supports)
s=r.state();s['resume_dispatch']['analysis_conversion']=['PART-507'];s['resume_dispatch']['independent_verifier']='ARCH-385';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
