import subprocess
import record as r
rows=[
('edgion-tests/src/client/suites/edgion_plugins/wasm/wasm.rs','a9f3018afacc7d245ea339feb5ec0c6d2549bd18ca23fb3291d4e390082c6db3','Complete427-520 target/preceding request callouts/registered vector523 through EOF.'),
('edgion-tests/wasm/header-demo/src/lib.rs','427be5953df582275faaa20a15f86c7c75a6a440134653c7c23dc8583e7b60bb','62-157 CAS/rootconfig/tick/callback and183-225 read probes.'),
('edgion-gateway/src/plugins/http/wasm/vm_manager.rs','c4bb5992f1a65039adf2dd11c80c517c584d04a69ae5790ec92d1be93dd1336e','395-505 owner/shutdown/518-563 reuse/1198-1325 lease-cancel completedfollowup/1340-1368 sharedstore injection.'),
('edgion-gateway/tests/unit/plugins/http/wasm/vm_manager_test.rs','803d79693ef58fca26908efeeff437db44213d7ffdd75778bc62ab2411a11335','Actual492-534 isolation/1566-1583 sharedvisibility/1700-1735 fresh tick/1790-1852 callback success-failure.'),
('edgion-tests/integration/conf/EdgionPlugins/Wasm/02_EdgionPlugins_wasm_advanced.yaml','cadb49dced6288e8004544837507b1e6686dcab77166b75b031ad601b72cf361','Full local owning origin/grant/stage/deadlines.'),
('edgion-tests/k8s/conf/EdgionPlugins/Wasm/02_EdgionPlugins_wasm_advanced.yaml','9a00b7ae03e8b3e794199ae404236e796e2c67ab780360655c0a02872367bd10','Full K8s owning origin/grant/stage/deadline; stale sharing comment not ownership proof.'),
('skills/04-review/wasm/regression-guards/wasm-body-counter-delta-rerun-safe.md','d96a18addabe55c9f7248b00badf78aee44b6087df01f4ed784e26aac349a944','Full original explicitly body-scoped delta/reset/repeat guidance.'),
('skills/04-review/wasm/regression-guards/wasm-module-cache-key-runtime-config.md','7ad0b22400357d0aea1790d072ff36415fd5bb7ad2739b939b19866dbae6ba9a','Full configured-instance/generation/stage isolation vs historical sharing.')]
supports={}
for p,h,scope in rows:assert r.digest(r.REPO/p)==h,p;supports[p]=scope
r.confirm('ARCH-401','/root/verify_ip_union','7ed8f8920605884bcc5431eb3d0f63740e49c346e2481971ede918d7bb471ea5','2aa56cc69874dca8dd24f3b5d7fdc6948a989362ad130e2a14dc22e16cf33eca',[
'Independent unchanged cumulative configured-runtime counters/current probes reuse persistent owner; earlier callback200 can satisfy later threshold despite no new success.',
'Bounded baseline/positive checked delta with numeric/reset/deadline failures feasible without guestreset/runtime replacement/production instrumentation. Real fresh-runtime tests do not prove this integration window.',
'Body-counter guidance remains body-scoped. Own original rules/index/graded/routing/quiescence/pages/bothroutes freshly personal unchanged. No duplicate/no execution/handshake or scheduling incident.'],supports)
s=r.state();s['resume_dispatch']['analysis_outbound']=['PART-524','PART-525','PART-526'];s['resume_dispatch']['independent_verifier']='ARCH-348 KeyAuth supplemental';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
