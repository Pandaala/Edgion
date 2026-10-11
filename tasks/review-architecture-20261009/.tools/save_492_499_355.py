from compact_receipt import persist
import record as r
import hashlib,subprocess
persist('PART-492','/root/analyze_outbound',{
'ip_restriction.rs':('23910a9a6525d468fa88f5577c1ad3897ad6fd2bef40c20216b23fb0e64ed1cc','Full1-142','Four real HTTP allow/deny hit/miss cases preserve canonical listener and exact403/200, transport failure fails. Local/K8s peer fixtures intentionally differ. Accepted.'),
'mod.rs':('b363371c7e099f94e64fc8df0c3a47c175771e50d77a6efbda87c5219f8ad106','Full1-5','Private implementation/export real registry, no issue.')},[
('ip-source-client-ip-false-positive.md','3af25d0bcba626e03915f0ad36fb5f0f8beae6d43321e27cd90b65739a469582','Full1-61 original verified current pinned request snapshot resolver.'),
('ip_restriction/plugin.rs','f2737aa2f7772cafc79af9a744e3e356ded91d4212b1e30008b124d10deaab7b','Full1-259 actual production owner.'),
('ip_restriction/plugin_test.rs','9c5583b790b5cbe3980c25b62e1ee3aba49de966dcac27026260394ff99ba871','Full1-693 constructor/directpeer/missingIP/refdeny/reload/ownership units.'),
('IpRestriction/HTTPRoute_default_ip-restriction.yaml','03649f5f757b1465c359da39fe7f52916fca4016f57da4d16fe43519c4e92e05','Full1-76 local route.'),
('IpRestriction/HTTPRoute_default_ip-restriction.yaml','8ae8267b7efc608d5b16ac97562f3c33e58c795e01ae75e16d1c6b1e42079b25','Full1-68 K8s route.'),
('test_client_peer_identity.sh','e0034731a4c284260279d10e14ce252d243cb689df71d3e407f3a5c1c914c9ad','Full1-156 ReadyPod/UID/IP/CIDR publication/stale rejection.'),
('test_client_peer_identity_test.sh','bcd67589830dfbaf0abf0ccbaf24ebd6350c79cff98e8313a50abd6dff08f001','Full1-134 actual IPv4/IPv6/stale/zero/multiple/invalid/termination tests.'),
('run_k8s_integration.sh','9a858cd81d47c74f20b375a503b10f1c52107ab413116783c8a49c4ec16e42e1','2898-2942,2985-3018 publish before restart and assert before normal/reload.')],
'Static only/no execution/mutation/network/physical topology claim. Both five-file fixture trees+README/baseEGC/registration2770-2784/adapter1491-1510/resolver505-542/resource source+validation+allowintent/evaluator actual read. Original constructor/group/evaluator rules personally checked; old source spelling historical. No new duplicate mapped-CIDR/borrowedmatcher cause. Canonical log/redaction/restraint/body/TLS preserved, support zero.')
persist('PART-499','/root/analyze_conversion',{
'ldap_auth.rs':('cbe1d3686e5ab078c9659dd1439a57b76c8aeddf3997c377f5ddacc4dd6c67cd','Full1-467 three chunks,21282bytes','Ten real missing/invalid/wrongcredential/proxy/basic/anonymous/hide+identity/RFC4514space+plus binds, no new root. Positive identity prevents defaulted response false PASS.'),
'mod.rs':('1cefc2f8a869cb372a06618f080339ddafc8a0e1d231dc32bfbd209a79315a61','Full1-18,1061bytes','Actual export/registry and fixture scope accepted.')},[
('ldap_auth/plugin.rs','d4a7d9f9d41bd609de65f0a3fe2d470f24e375b42450e0b19bf3d6504c02e206','240-597 TLSproof/current auth/dial/rc49/generic503.'),
('ldap_auth/plugin_test.rs','2f34688a68c69dbc594934409d7d38e32346df53b6ceea6086f80df91319680c','47-194,293-337 real parse/proxy/escape/anonymous units.'),
('','bdd5ef76e01a73ec8bd3692d5cf87e3a40ad993460d0ed56d888bc9a94d10b36','Full1-18 local LDAP base.'),
('','b24dc28663e35e12bc86597813c07c568951dc2b54e2054f934b5a29e40a3a50','Full1-18 K8s LDAP base.'),
('','a145db9c1ce3323ec79211bc5ab68fae41db71727a78a3969ce6be238c49cb6c','Full1-34 actual local LDIF producer.'),
('','402e45cf59a97b8d3288a408e553793af6eca187398ae43fb92829edfe1dc98f','Full1-38 actual K8s seed.'),
('dial-time-egress-policy.md','dfa65cff757eafbde736d2392b5a23ba1a8249712eece63fa72134d464336d9c','Full1-184 original dial-time policy including bounded recovered middle.')],
'Full eight fixtures each topology+real LDIF/seed/deployment/compose/startservice; no missing-user/topology assumption. Per-request connect/cache removal accepted, no pooling/timing redesign. Connection/error-tag originals full/localrunner910-1028/slow registration/K8s2563/main2666-2679 inspected. Retainedcontainers exception standalone scoped, not imported shared runner. Syntheticcredentials not realleak. Original rules/no nested personally read/revalidated, gates satisfied. Static no execution/mutation; supports zero.')
body='''
# The managed-overlay suite treats structural acceptance as proof that the overlay reference resolved

`ep_resolves_pm_chain` and `pm_validated_accepted` in `edgion-tests/src/client/suites/edgion_plugins/edgion_config_data_managed/edgion_config_data_managed.rs` poll the EdgionPlugins resource and pass when its selected Controller conditions contain `Accepted=True`. The first success message explicitly claims that `overrideRef` resolved correctly; the second case presents acceptance with pre-created ConfigData as evidence of the managed reference chain. Neither predicate checks `ResolvedRefs`, nor verifies that the plugin actually references the expected ConfigData. The preceding ConfigData checks establish that `data-managed` or `data-validated` exists; they do not establish the plugin's reference edge.

The current Controller deliberately separates these facts. `EdgionPluginsHandler::update_status` constructs Accepted from structural/preparse validation, then separately calls `resolve_override_refs` and constructs ResolvedRefs. A missing, disabled, denied, wrong-type or invalid effective overlay can therefore coexist with an accepted safe base. The actual `test_validate_region_route_override_ref_missing` unit requires missing-overlay preparse to remain clean, parse to continue and reference resolution to report `EdgionConfigDataNotFound`. The original ConfigData architecture likewise requires fail-to-base rather than structural rejection. These are supported states, not malformed diagnostic responses.

A concrete regression witness is changing a fixture plugin's `overrideRef.name` to another syntactically valid, absent name while leaving the pre-created expected ConfigData and valid base unchanged. The existence checks still pass and Accepted can remain True while ResolvedRefs is False. All three managed-overlay cases can then pass despite the claimed managed reference chain being broken. The committed local and Kubernetes fixtures both supply explicit expected references in `edgion-default`; neither topology turns Accepted into a reference-resolution condition. This is distinct from ARCH266: these readers already use the current canonical Controller-local conditions projection.

Within these two existing cases, inspect the returned plugin's actual RegionRoute entry and require the expected override reference identity. In the same bounded poll, require both `Accepted=True` and `ResolvedRefs=True` from the already-selected Controller observation. When `metadata.generation` is present, require both conditions' `observedGeneration` to match it; preserve native FileSystem/etcd observations where no generation is supplied. Preserve the existing expected ConfigData presence/type checks, authenticated Admin requests, native and partitioned status handling, retry limits and fail-on-read/parse behavior. Missing conditions must keep the predicate unsatisfied. No production status, fallback, admission, logging, fixture ownership or generic test framework changes are needed.

Checking Accepted alone cannot prove the edge, and checking only ConfigData existence cannot detect a mistargeted reference. A traffic-success check alone would also admit the intentionally valid base fallback. Future verification should feed the real predicates healthy, missing-target, mistargeted-reference, stale-generation and absent-condition observations without changing the existing test identities. This review is static; no suite or cluster was executed.
'''
assert hashlib.sha256(body.encode()).hexdigest()=='2db3cbedebc04a3ac586b1b892e62656e810157e40c4c10f52f60ebc26c54008'
rows=[
('edgion-tests/src/client/suites/edgion_plugins/edgion_config_data_managed/edgion_config_data_managed.rs','8e2b8745dfff2e82c8199e4d70d512aa6b9d4f606bfdf6e6b57e0243e6520222','Full source cases/existence/predicates/auth/polls/messages.'),
('edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs','9a6ed6dbb4caa2a8a6a5b9bee0e9a9d75344edd9aad3b9eeee5dccc6413dc16a','140-180,411-502,535-582,1050-1097 marking/independent status/generation/effective validation.'),
('edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins_test.rs','f50b5c4d0c2b9ab687f84f4e688e9eb34dc64ae61e03589f49d87407ef172ccd','3170-3243 actual complete missing-override unit.'),
('edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/config_data_ref_mark.rs','908c316fcf483af503eb1903a36984256fe083d549f38e8b9f80369cbecfa602','Full identity/availability/type/permission/disable ordering.'),
('edgion-tests/src/client/suites/status/helpers.rs','0236de331bc053f9ba00610543ef4e7a8932a9d49e129110f068091d98f823dc','1-82 canonical native/controller projection and actual unit bodies.'),
('edgion-tests/integration/conf/EdgionPlugins/EdgionConfigDataManaged/01_EdgionPlugins_data-managed.yaml','6f7c2fcae611e83b264af98bdb95a7360332f0f7166e99781e65eed89ae9e77e','Full local fixture and personally other expected targets/plugin inspected.'),
('edgion-tests/k8s/conf/EdgionPlugins/EdgionConfigDataManaged/resources.yaml','280beba7e2b6d0233acc0315db1330e92f92ec2e8bb3224e25a5168256996691','Full target/pluginref/attachment fixtures.'),
('skills/01-architecture/05-resources/15-edgion-config-data.md','fa43b5425b617f7e7a640f182dd527721d77dc939b56d4a2d0d25cb33c42d02e','65-113 safe base/matrix/consumer vs standalone validation.')]
supports={}
for p,sha,scope in rows:assert r.digest(r.REPO/p)==sha,p;supports[p]=scope
r.confirm('ARCH-355','/root/verify_ip_union','3a015e70a398abc48c005f07980d804bbb52232687785f447b5969eb15d58475','e33e2d2edbb799c52fa0fc7a85504aef251458558556bef59751770a317eb25e',[
'Independent missing-reference witness survives; structural Accepted and reference ResolvedRefs intentionally separate.',
'Exact approved revision qualifies second case message and preserves generation-less FS/etcd observations.',
'Original rules/status management/serialization/constructors/effective-table/admin actual code personally inspected; no tests or execution.'],supports,replacement_body=body)
s=r.state();s['resume_dispatch']={'analysis_conversion':['PART-500','PART-501'],'analysis_outbound':['PART-496','PART-497','PART-498'],'independent_verifier':'ARCH-348-pending-supplements'};r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
