import json,subprocess
import record as r
d=json.loads((r.RUN/'VERIFICATION-ARCH-393.json').read_text());supports={}
scopes={
'edgion-tests/src/client/suites/edgion_plugins/request_mirror/request_mirror.rs':'Polling/marked selector; complete400-437 path/query and662-713 query cases; eleven-case vector.',
'edgion-tests/src/server/test_server.rs':'Complete MirrorCapture/capture+query1041-1117/slow1125-1165; both constructors and serialization.',
'edgion-gateway/src/plugins/http/request_mirror/plugin.rs':'249-275 marker and291-435 exact URL path/query/committed vs streaming ownership/discarded response.',
}
for p,e in d['support_selected'].items():
 if p.endswith('body_plugins.rs'):continue
 assert r.digest(r.REPO/p)==e['sha256'],p
 supports[p]=scopes.get(p,e['scope'])
p='edgion-gateway/tests/unit/plugins/http/request_mirror/plugin_test/arming_tests.rs';assert r.digest(r.REPO/p)=='5826dfaa49e74fa1a0965b352632d4933b3f561a4ee0194c8925dc304447d1cc';supports[p]='84-105 prepared asserts/query=None211-229/actual300-355 snapshot/framing/empty-completion bodies.'
r.confirm('ARCH-394','/root/verify_ip_union','af6253701af74b61acbf4923cc245ead55c369bf8fd66e491a994cf6103586c5','f73d32b11f78673082090c62a63d64e62af842e353b0dcf1814b44d86f8f54f8',[
'Independent unchanged named assertions omit query; dedicated case already marked-selects so393 alone cannot fix it. Schema/both constructors discard query and serialization cannot recover it.',
'Omitting current production get_query branch preserves present asserted marker/path. Test-only field in both constructors plus exact selected target is feasible without production instrumentation.',
'Real current arming units use query=None, establish useful different boundaries. Original root/index/graded/routing/mirror budget/snapshot/no-network pages personally read and fresh hashes unchanged. Distinct382/393; no observed production loss or execution.'],supports)
s=r.state();s['resume_dispatch']['independent_verifier']='ARCH-401';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
