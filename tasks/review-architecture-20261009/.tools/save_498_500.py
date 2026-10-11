from compact_receipt import persist
import record as r
import hashlib
body='''# Make the LDAP TLS Kubernetes mTLS case reach a certificate-required server

The registered `mtls_client_cert_bind_succeeds` case in `edgion-tests/src/client/suites/edgion_plugins/ldap_auth_tls/ldap_auth_tls.rs:101-139` describes a server that requires a client certificate and passes on HTTP 200. Its Kubernetes fixture does not establish that prerequisite. `k8s/conf/EdgionPlugins/LdapAuthTls/11_Deployment_edgion-test_openldap-tls.yaml:63-66` configures the single OpenLDAP server with `LDAP_TLS_VERIFY_CLIENT: "try"`; `12_Service_edgion-test_openldap-tls.yaml:10-17` directs both the CA-only port 33636 and the purported mTLS port 33637 to that same server's port 636. The mTLS plugin supplies a client identity, but its positive bind result cannot distinguish required identity presentation from a connection accepted without one.

This is a reachable test-evidence gap, not a production TLS bypass or a reported execution failure. The current Kubernetes runner strictly applies this fixture tree and explicitly registers LdapAuthTls. The CA-only sibling expects a successful bind without a configured client certificate against the same listener. Locally, the compose fixture already uses a distinct port-33637 server configured with `demand`, showing the intended boundary without requiring another Gateway or Controller. Gateway LDAP TLS construction separately selects `with_client_auth_cert` or `with_no_client_auth`; existing material-binding unit tests verify reference identity but do not establish this Kubernetes server requirement.

Give the Kubernetes mTLS fixture a genuinely certificate-required endpoint, using the existing synthetic CA, client identity and seeded users. Keep CA-only LDAPS and STARTTLS on their existing compatible endpoint. Wire the existing mTLS route/plugin to the required endpoint and add a narrowly scoped no-client-identity control that must fail while the matching identity succeeds. Review preparation/readiness and run-owned cleanup for the additional dependency within the existing shared topology. Merely renaming the case or enabling certificate requirements on the shared listener would respectively retain the gap or break the CA-only sibling.

Preserve all four case identities and assertions, strict Secret-reference preparation, wrong-CA fail-closed behavior, existing deadlines, credential handling and canonical outbound TLS resolution. No production TLS changes, Gateway rustls support, logging, metrics, generic test framework or extra request-body ownership is needed.

The original LDAP egress guard already records an independent required-mTLS with/without-identity matrix; that standalone coverage remains valuable and is not claimed absent. It does not make this separately registered Kubernetes assertion prove its stated prerequisite. ARCH283 concerns classifying negative probe failures and has a different acceptance owner. Future verification must establish the required endpoint rejects missing identity and accepts the matching identity in Kubernetes, while CA-only and STARTTLS siblings still pass. Static repository review only: external image internals and live handshake outcomes were not executed or fetched.'''
assert hashlib.sha256(body.encode()).hexdigest()=='686984cae6cee0438ff1178cc7c7813a81f1e840e14f700bdd2aa8ce07248d93'
r.candidate('ARCH-373','ldap-kubernetes-required-mtls-fixture','PART-500','/root/analyze_conversion','behavior-defect','P2','\n'+body+'\n')
persist('PART-498','/root/analyze_outbound',{
'key_auth.rs':('d2256be38233f10140f95a33ee590955089e4133266949e10a32452e7099e1e4','Full1-1053 four disjoint complete ranges','Twenty live cases sourcepriority/groups/echo/delay/dualaliases canonical logs. ARCH377 optional anonymous positiveidentity; ARCH348 unreadable credentialabsence separate pendingroot.'),
'mod.rs':('0c339766bb0219a929338309a4eda7f7e8394e186728aeba1ab726653da1bb3f','Full1-10','Private export/staticregistry, accepted.')},[
('05-testing/01-integration-testing.md','e260042b508fa0956873d1b3c059531f1733a7c15423c548081afb71703a10e1','Full original retained unchanged;fresh307-334 direct observation priority.'),
('key_auth/plugin.rs','a0879891ff652cda72d681764a398c7a4338d43dc8b5d30884f7c5ad12555940','Full1-214 truncated middle recovered104-220 beyondEOF.'),
('key_auth/plugin_test.rs','830050ba8074e39f98041c159199b2a8010b6c1d36024749121ed58ca4466c92','Full1-396 actual unit bodies.'),
('KeyAuth/02_EdgionPlugins_default_key-auth-anonymous.yaml','32ab7863567b44532a9293f80453a12423daf119bdd88800ceebb40f15c7ffc6','Full1-26 both topology anonymousfixture.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','881-938 complete /headers producer;retained3350-3427 query/cookie echo.'),
('resolve_credential/key_auth.rs','31204f72195dd040e198c8d6342d2e7dc8314c6bb0ff579a85295b8383b65051','Full1-229 actual typed credential resolver.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','2638-2647 actual registry.')],
'ARCH348 supplement451-503 hides credential assertion textErr->empty passes absence of x-api-key/test marker after200; current /headers completed observation required, no production stripping/leakclaim. Rules/index/graded/routing full personal freshhash/no nested; groupfirstwins/successonly/source-loop/strip/hash-timing originals and all32fixtures/Controller units fully read. Delay is wallclock integration softwarning, not measured delayfree success; pausedtime units separate. No execution/network/mutations/new instrumentation, support zero.',candidate_ids=['ARCH-377'])
persist('PART-500','/root/analyze_conversion',{
'ldap_auth_tls.rs':('3729698af350e6bc98c9e8e81fe01ceb7a3d724b70e52fa989e28811a057f9fa','Full1-195','Four registeredTLS cases; ARCH373 current K8s purported mTLS endpoint doesnotrequire certificate, others preserved.'),
'mod.rs':('6e88e5a4ec801aebe5e67a3974459d7439474ca8c3887ee163457bb56cf3b3ac','Full1-17','Live export accepted; stale inlinePEM wrongCA comment documentationonly.')},[
('LdapAuthTls/11_Deployment_edgion-test_openldap-tls.yaml','1ad82fa9dbc880149fb3867431d0eae5a10c8bd1ecf8f6cbc2b30201a4cf5556','Full1-113 try-policy server.'),
('LdapAuthTls/12_Service_edgion-test_openldap-tls.yaml','4f9576697fc3a3d4fbd8c4f62ba08dbf45035defa098ecdb22c57b007e02b5c0','Full1-21 shared listener target.'),
('LdapAuthTls/04_EdgionPlugins_default_ldap-auth-tls-mtls.yaml','1027cea6bdd4abce90986a419e98b69cafd6a6f632962f88309735b71c2bd306','Full1-25 K8s mTLS plugin.'),
('ldap-openldap-tls/docker-compose.yml','7e90ff478f6c62825b1784104c4a00afe6c029168ddeec1b88aeb2d7feca7e61','Full1-61 distinct local demand endpoint.'),
('run_k8s_integration.sh','9a858cd81d47c74f20b375a503b10f1c52107ab413116783c8a49c4ec16e42e1','1170-1275,2572-2584,2830-2900 actual strictapply/readiness/registry.'),
('apply_all_conf_strict.sh','394ea219c45f7bbd72f2cfc346655215e49367dde3e0d057de565f376adb2c72','1-150 fixture filter/copy preservescontent.'),
('ldap_auth/plugin.rs','d4a7d9f9d41bd609de65f0a3fe2d470f24e375b42450e0b19bf3d6504c02e206','240-597 TLSidentity/nativebind/fixed503.'),
('dial-time-egress-policy.md','dfa65cff757eafbde736d2392b5a23ba1a8249712eece63fa72134d464336d9c','Full1-184 requiredmtls matrix/scoped retained standalone exception.')],
'All eight plugin/route YAML each topology,three local/fourK8s Secret YAML,LDIF34/make-test-certs166/real encryptedSAN+material units600-657,837-957/localrunner1030-1170/main2666-2679 and original ARCH283 body24 read. Existing negativeprobe cause distinct. Personal rules originalcurrent/nested rootonly and gates satisfied. Staticonly/no externalimage/handshake/profile execution/network/mutation/support credit.',candidate_ids=['ARCH-373'])
s=r.state();q=s.setdefault('pending_supplements',{}).setdefault('ARCH-348',[])
if 'REPORT-PART-498.json' not in q:q.append('REPORT-PART-498.json')
s['resume_dispatch']['analysis_outbound']=['PART-502','PART-503','PART-504'];s['resume_dispatch']['analysis_conversion']=['PART-501'];r.save(s)
