"""Save completed small harness batches and pending same-root supplements."""
import subprocess
import record as r

def save(ident,analyst,primary,rows,limits):
    if r.state()['partitions'][ident]['status']=='reviewed':
        return
    dispositions={}
    for p in r.state()['partitions'][ident]['paths']:
        prefix,desc=primary[p.rsplit('/',1)[1]]
        assert r.digest(r.REPO/p).startswith(prefix),p
        dispositions[p]=desc
    supports={}
    for p,prefix,scope in rows:
        assert r.digest(r.REPO/p).startswith(prefix),p
        supports[p]=scope
    r.partition(ident,analyst,dispositions,supports,[],limits)
    subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)

save('PART-475','/root/analyze_outbound',{
 'dynamic.rs':('d2b26c55','Full1-288 six phase cases; selected Controller conditions distinguish structurally accepted missing overlay from ResolvedRefs failure/recovery; no new issue.'),
 'edgion_config_data_status.rs':('2b6d11bd','Full1-259 ConfigData native status intentional; invalid StreamPlugins uses selected conditions and exact Invalid/activeProfile; no issue.'),
 'mod.rs':('24bc45af','Full1-6 actual phase/static exports and registration; no issue.')},[
 ('skills/04-review/status/regression-guards/deployment-partitioned-observations.md','f34036a4','Full original; ConfigData deliberately excluded from isolated kinds.'),
 ('edgion-resources/src/resources/edgion_config_data/mod.rs','8f124494','192-225 actual native ConfigData status.'),
 ('edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data.rs','e8dfb8e3','233-330 actual Accepted/ResolvedRefs producer.'),
 ('edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data_test/unknown_fields_tests.rs','eba5fc8f','Full1-91 actual status preservation/invalid policy units; corrected scope.'),
 ('edgion-tests/src/client/dynamic/scenarios/edgion_config_data_ref_flap.rs','4587ccd9','Full1-60 actual six-phase update orchestration; existing ARCH264 restoration scope.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/EdgionConfigDataStatus/01_EdgionConfigData_data-status-gcir-valid.yaml','341c9f4a','Full selected Selector fixture.'),
 ('edgion-tests/src/client/suites/status/helpers.rs','0236de33','Retained personally full1-244 freshly unchanged native/selected projection and units.')],
 'Complete static/no execution/mutations. Six local/six Kubernetes fixtures and registration2888-2929 read. Native ConfigData status falsifies apparent ARCH266 issue; no supplement. Original ConfigData/native GCIR validation and authority personally read/current-checked. Existing ref-flap lifecycle belongs ARCH264, no duplicate. Supports zero coverage.')

save('PART-478','/root/analyze_conversion',{
 'fault_injection.rs':('0d124e5e','Full1-428 eight delay/abort/condition/zero-ratio/204/canonical log cases; no distinct issue. Same ARCH296 fault_url10-12 literal31180 scope extension pending.'),
 'mod.rs':('ba90c6b5','Full1-5 export/registration2796-2802 checked; no issue.')},[
 ('edgion-gateway/src/plugins/http/fault_injection/plugin.rs','7b7d5966','Full actual plugin.'),
 ('edgion-gateway/tests/unit/plugins/http/fault_injection/plugin_test.rs','cc7649c5','Full actual seeded/no-sample deterministic unit contracts.'),
 ('edgion-gateway/src/plugins/runtime/log.rs','bb8bd222','138-203 actual merged array serde.'),
 ('edgion-tests/src/client/access_log_client.rs','f7691b8a','320-397 canonical marker/get/retry.'),
 ('edgion-tests/integration/conf/EdgionPlugins/FaultInjection/HTTPRoute_default_fault.yaml','37214f75','Full local selected route.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/FaultInjection/HTTPRoute_default_fault.yaml','0d6c538f','Full Kubernetes route.')],
 'Complete read-only context; all seven plugins in both modes and local README read. No ARCH344 projection gap: actual log serde and canonical marker queried. Bodyless204 does not establish reachable read-error witness for ARCH348. Coarse timing integration bands are not measured performance evidence. Rules personally revalidated, no nested AGENTS; no execution/mutation/network; supports zero primary coverage.')

save('PART-479','/root/analyze_conversion',{
 'mod.rs':('41ed1891','Full1-92 two phase-specific exact sorted-body assertions use canonical mutation; no distinct issue. Same ARCH296 literal31180 at26/69 and ARCH264 scenario lifecycle scope pending extensions.')},[
 ('edgion-tests/src/client/dynamic/scenarios/form_json_transform.rs','eac9c0ae','Full actual initial assertion/update/wait, absent initial apply/restoration at29-36.'),
 ('edgion-gateway/src/plugins/http/form_json_transform/plugin.rs','44eadd05','Full1-423 actual canonical transformation.'),
 ('edgion-gateway/tests/unit/plugins/http/form_json_transform/plugin_test.rs','118cf4fb','Full1-221 actual transformation units.'),
 ('edgion-tests/integration/conf/EdgionPlugins/FormJsonTransform/DynamicTest/initial/EdgionPlugins_form-json-transform-dynamic.yaml','8e87ba94','Full local initial direction fixture.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/FormJsonTransform/DynamicTest/initial/01_EdgionPlugins_form-json-transform-dynamic.yaml','8e87ba94','Full Kubernetes initial fixture.')],
 'Complete static analysis; both routes/update directions and actual registration/runner TSV34 inspected. Exact nonempty echo comparison cannot pass an empty failed observation, no ARCH348 extension. Preserve both valid direction assertions; lifecycle fix belongs scenario owner. Existing ARCH029 production classifier cause deduplicated. No execution/network/mutation; supports zero credit; original rules/topic decisions personally checked.')

save('PART-480','/root/analyze_conversion',{
 'forward_auth.rs':('bae663b7','Full1-1654 seven consecutive bounded ranges,26 registered cases; auth/probe/multivalue denial/body/boundedfailure preserved. Pending same ARCH348 strict204 case observation extension.'),
 'mod.rs':('7920bf6c','Full1-15 actual modules/export registration; obsolete combined-route comment only documentation follow-up.')},[
 ('edgion-gateway/src/plugins/http/forward_auth/plugin.rs','50d0e604','Full1-366 actual auth decision/denial relay.'),
 ('edgion-gateway/tests/unit/plugins/http/forward_auth/plugin_test.rs','08d4e06f','Full1-495 exact denial bytes/header order/shutdown unit bodies.'),
 ('edgion-tests/src/server/test_server.rs','6c38b285','750-770,880-1015,4370-4675 health/auth JSON/denial/header probes.'),
 ('edgion-tests/src/client/framework.rs','dd7b89a2','103-127,536-590,600-648 five-second HTTP/thirty-second wrapper.'),
 ('edgion-tests/integration/conf/EdgionPlugins/ForwardAuth/17_EdgionPlugins_forward-auth-strict-204.yaml','6382d886','Full1-32 local exact204 allowed-status fixture.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/ForwardAuth/17_EdgionPlugins_forward-auth-strict-204.yaml','6382d886','Full1-32 Kubernetes exact204 fixture.'),
 ('skills/04-review/forward-auth/regression-guards/deny-response-repeated-headers-preserved.md','7cbb9212','Full1-100 original denial relay ownership.'),
 ('edgion-gateway/src/plugins/http/common/outbound/client.rs','f74828d2','512-550 actual custom status classification.'),
 ('edgion-resources/src/resources/common/success.rs','d5cdcba1','145-204 configured success status semantics.')],
 'Complete read-only terminal;21 local/21 Kubernetes YAMLs and both actual runners/context inspected. Same-root ARCH348 extension pending independent review: strict204 case1445-1512 defaults HTTP200 body at1469; loose200/not-backend branch1487-1498 accepts unreadable/empty/unrelated body without auth identity. Fallible read and actual expected auth JSON identity/status are bounded remedy; legitimate empty204 sibling unchanged. No new finding/production defect/normal fixture outage. No ARCH296 because ctx.http_url used. LB case smoke-only limitation retained. No execution/builds/network/mutation; supports zero credit.')

state=r.state()
for ident,reports in {'ARCH-296':['REPORT-PART-478.json','REPORT-PART-479.json'],'ARCH-264':['REPORT-PART-479.json'],'ARCH-348':['REPORT-PART-480.json']}.items():
    queue=state.setdefault('pending_supplements',{}).setdefault(ident,[])
    queue.extend(p for p in reports if p not in queue)
state['resume_dispatch']={'analyst_conversion':['PART-481','PART-482','PART-483'],'analyst_outbound':['PART-475','PART-476'],'independent_verifier':'ARCH-001 current AI/WAF capabilities','note':'ARCH348 approved original+PART464 merged; PART480 further same-root extension pending independent review. ARCH001 residual-AI receipt archived historical before concurrent AI extraction.'}
r.save(state)
