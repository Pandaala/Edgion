import json,subprocess
import record as r
from compact_receipt import persist
body='''
# Require fresh Wasm tick progress in the observation window

`test_tick_shared_data` and `test_tick_callout` in `edgion-tests/src/client/suites/edgion_plugins/wasm/wasm.rs:427–520` sleep and accept a returned counter whenever it is at least one. They describe live tick execution and a completed tick-phase callout, but these counters belong to the configured Wasm plugin lifetime. The reference guest cumulatively increments `ticks`; its root callback increments `calloutticks` only for an HTTP 200. Request probes merely read those shared values. Current plugin-owned runtime isolation retains the same shared-data store across ordinary requests without resetting it between suite invocations.

After one successful background callback, an unavailable auth dependency can stop subsequent successful callouts while the old `calloutticks` value remains positive. A later invocation of the registered tick-callout case still reports current success. Likewise, an unchanged positive tick counter does not prove another tick during this case. These are conditional observation witnesses, not a reported outage or a claim that current production scheduling is broken. Earlier request-phase callout cases and real fresh-runtime tick unit tests remain useful, but they do not turn an old cumulative value into evidence of progress in this case's window.

Read a successful baseline with the existing guest marker/header, then poll the same counter under a bounded deadline until a later successful probe shows a positive checked delta. Require numeric headers and fail on malformed observations, counter reset or a non-growing value at the deadline. Preserve both independent tests, their actual request-stage runtime and fixture host, valid auth origin and grants, HTTP outcomes and JSON shape assertions. A small private counter probe can reuse the present client and observation headers; no guest reset endpoint, fresh Gateway requirement or production instrumentation is needed.

Longer sleeps or a larger absolute minimum are inadequate alternatives: old state can satisfy either. Resetting counters hides lifetime behavior and mutates shared runtime state solely for testing. The original body-counter delta guard is explicitly body-scoped; its verified cumulative-state rationale supports this proposal without claiming it already required or implemented tick deltas. Current instance-isolation and owner-shutdown decisions remain unchanged.

Future verification should fail with a positive unchanged counter, accept later growth from a nonzero baseline, reject missing/reset counters and preserve repeated local/Kubernetes execution on the shared topology. Retain actual tick scheduling, callback failure handling, cancellation, epoch deadlines, canonical bounded egress and secret-safe observability; do not add metrics or request-path copies for mocks. Current historical successful runs do not verify the false-pass branch. All primary code and decisive guest/runtime/fixture/unit/original seams were personally read; no tests or external actions were executed.
'''
r.candidate('ARCH-401','wasm-tick-counter-window','PART-517','/root/analyze_conversion','behavior-defect','P2',body)
persist('PART-517','/root/analyze_conversion',{
'mod.rs':('e8ba4063703d02e295f591289e59db3990ac580913ca6334fc0e1959c9bb79b2','Full1-3','Export accepted.'),
'wasm.rs':('a9f3018afacc7d245ea339feb5ec0c6d2549bd18ca23fb3291d4e390082c6db3','Full1-541 three chunks','All9 registered guest/header/localreply/trap recovery/callout grant/tick cases read.401 lifetime-counter observation gap; performance smoke only stated header/mean, no payload guarantee.')},[
('header-demo/src/lib.rs','427be5953df582275faaa20a15f86c7c75a6a440134653c7c23dc8583e7b60bb','60-145/190-218 actual cumulative CAS/root/callback200/probe reads.'),
('wasm/vm_manager.rs','c4bb5992f1a65039adf2dd11c80c517c584d04a69ae5790ec92d1be93dd1336e','660-699/1187-1273 perentry shareddata/persistent task/lease/callout.'),
('wasm/vm_manager_test.rs','803d79693ef58fca26908efeeff437db44213d7ffdd75778bc62ab2411a11335','1395-1498/1700-1720/1790-1869 actual guest/driver/fresh runtime/tick/success/failure units.'),
('Wasm/02_EdgionPlugins_wasm_advanced.yaml','cadb49dced6288e8004544837507b1e6686dcab77166b75b031ad601b72cf361','Full1-51 local request origin/grant vsresponse.'),
('Wasm/02_EdgionPlugins_wasm_advanced.yaml','9a00b7ae03e8b3e794199ae404236e796e2c67ab780360655c0a02872367bd10','Full1-53 K8s DNS origin/grant; stale sharedcache comment prose limit.'),
('wasm-body-counter-delta-rerun-safe.md','d96a18addabe55c9f7248b00badf78aee44b6087df01f4ed784e26aac349a944','Full1-68 original explicitly body-scoped decision/cumulative rationale.'),
('wasm-module-cache-key-runtime-config.md','7ad0b22400357d0aea1790d072ff36415fd5bb7ad2739b939b19866dbae6ba9a','Full1-87 current instance/generation isolation.'),
('tick-quiescence-on-zero-reference-eviction.md','9e05680dec79c5db7d5458b80bf079f3ee3e7faba39197d8e7a149bbce30febc','Full1-78 recovered after truncated combined output; original cancel/publication/lease/epochs.')],
'All4local/5K8s static YAML full; registry2740-2745/wrapper300-317/auth server4545-4627/body callback original read. Personal original rules/index/graded/routing/topics fresh unchanged. No duplicate current runtime ownership root; bounded401 observation only. No execution/writes/compiled-artifact equivalence/physical restart claims; support primary0.',('ARCH-401',))
rows=[
('edgion-tests/src/client/suites/edgion_plugins/request_mirror/request_mirror.rs','87eff33e87429f292ffe7b1b70f8b3addd6bbf13917ee6c111bf14a7d29805b7','76-143 helpers/completePOST210-273/path400-437/eleven-case vector.'),
('edgion-tests/src/server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','40-51/1041-1117 actual capture/query handlers insertion-order array.'),
('edgion-tests/integration/conf/EdgionPlugins/RequestMirror/01_EdgionPlugins_request-mirror-basic.yaml','d7998a8ea0985645a6c71b15af53c1227bffc940786e723572a98bea0c7f6993','Full local mirror destination.'),
('edgion-tests/k8s/conf/EdgionPlugins/RequestMirror/01_EdgionPlugins_request-mirror-basic.yaml','cddd732d784a611a05d7601190d4ec16183333998309aa8d05fe0c2b898c04d1','Full K8s mirror destination.'),
('edgion-tests/integration/conf/EdgionPlugins/RequestMirror/HTTPRoute_default_mirror-basic-route.yaml','f894105fe345cccfce718a204d47b6fb9c831f769b6d2de1ef9d046ec2f32ed0','Full local primary route same capture backend.'),
('edgion-tests/k8s/conf/EdgionPlugins/RequestMirror/HTTPRoute_default_mirror-basic-route.yaml','eef4153bdc07219cc778e7ed8df3ead9be0543053ae157d79f9defef2f532241','Full K8s primary route same capture backend.'),
('edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs','b0c929507e00960526e7edf1839a3154dc088ccebc58fa82801610ac7c6dc90f','213-267 selector/poll and1243-1302 exact committed mirror body assertion.'),
('edgion-gateway/src/plugins/http/request_mirror/plugin.rs','fd1f288cef9740573b8c73733a7fab83b22da97ef548c9ccff54d4f605b0e2ad','249-275 headers/correlation,291-435 snapshot vs streaming writer.')]
supports={}
for p,h,scope in rows:assert r.digest(r.REPO/p)==h,p;supports[p]=scope
r.confirm('ARCH-393','/root/verify_ip_union','57990ba078e9510971e1bbb78d49a776c3061d03f4f30f66f1e909712cbacf35','73d04a148c43984a6aa7700fccc20445e587ff72c2c6f1ec98955ab142c77058',[
'Independent unchanged some-marked poll returns entire array; both actual primary/mirror destinations share arrival-ordered store, no mirror-first promise.',
'Correct primary first plus marked wrong-body second satisfies current assertion; missing mirror remains failure. Existing selector/committed suite provide bounded correction.',
'Original root/index/graded/routing/mirror budget/snapshot/no-network pages and actual arming unit300-355/dispatch personally read.393 identity distinct394 query; no execution.'],supports)
f=r.RUN/'VERIFICATION-ARCH-390.json';d=json.loads(f.read_text())
scopes={
'edgion-gateway/src/plugins/http/region_route/plugin.rs':'224-309 complete redirect/route_to producers and406-442 direct selection/terminal call.',
'edgion-tests/src/client/dynamic/scenarios/region_route_validation.rs':'Complete407-490 check_traffic exact route/resource/plugin/endpoint/typed security/one-hop; FS/K8s distinction.',
'edgion-gateway/tests/unit/plugins/http/region_route/plugin_test.rs':'Actual266-324 one-hop/self/missing and455-557 accepted/local/rejected jumps.',
'edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs':'External dispatch/complete1543-1585 try_external_jump_peer and1673-1706 classification/error.',
}
for p,scope in scopes.items():d['support_selected'][p]['scope']=scope
f.write_text(json.dumps(d,indent=2)+'\n')
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
