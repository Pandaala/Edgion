"""Persist completed reports and approved symbol correction; reconcile task-only drift."""
import record as r
import subprocess,json,hashlib
paths=subprocess.check_output(['rg','--files','edgion-gateway','edgion-resources','edgion-controller','edgion-tests','skills'],cwd=r.REPO,text=True).splitlines()
def supports(rows):
 out={}
 for suffix,sha,scope in rows:
  matches=[p for p in paths if p.endswith(suffix) and r.digest(r.REPO/p)==sha]
  assert matches,suffix
  for p in matches:out[p]=scope
 return out
def checkpoint():subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
def part(i,analyst,primary,rows,limits):
 d={}
 for p in r.state()['partitions'][i]['paths']:
  sha,desc=primary[p.rsplit('/',1)[1]];assert r.digest(r.REPO/p)==sha,p;d[p]=desc
 r.partition(i,analyst,d,supports(rows),[],limits);checkpoint()
part('PART-489','/root/analyze_conversion',{
'guardrail_encoding.rs':('f40d51c4b7427c46babaf273fb05bf60359c0d4bc78fbdd0f36e9361dd2f48bb','Full1-287 split1-170/171-287,11771bytes; two live wire cases preserve repeated identity+gzip shape, fallible complete body reads, original SSE fail-open and503/no-provider fail-close, zero calls across endpoints and canonical aggregate reason. Accepted.'),
'mod.rs':('900f23349d4be9da08b8db5a4c79623ed3ebd707ed2d4ff05527ec3a79da0658','Full1-3,81bytes actual export/registration; accepted.')},[
('semantic/plugin.rs','4b7161bf80d9f0b9a8ef1ca07e1f38afe7f06a092b28a60f4a9fb6c3f6c4268e','Full1-189 current ai capability/canonical classifier/bodyless503 owner.'),
('proxy_http/content_type.rs','40c85de7a8e61579f243733dc82ac8197007dc523226e681405d6d649f043b61','Full1-118 identity classification.'),
('proxy_http/content_type_test.rs','2a4ed2f1b5201e06c899bf09a3cb0a8502ed1a1b9e2f4646472237eee6bc0a90','Full1-130 actual repeated/list/case/empty/nonUTF8 units.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','2366-2458 and retained1696-1817 raw SSE/repeated headers/observable call endpoint.'),
('','3e7856b3abf9e90bd04665ae7dedf140571aa29893d5867ffbbfbe1f16472c94','Full1-57 local policy.'),
('GuardrailEncoding/resources.yaml','7508791759f997af2afe0c77484264129a641cfe374e979cdf09703ff53a56c3','Full1-101 Kubernetes fixtures.'),
('k8s/conf/EdgionPlugins/base/AiGateway.yaml','d3bfe990eec9e145ad20da5f1f77078fbd13ab7c77a99354ec037746f6dca920','Full1-38 real shared fixture-provider owner.'),
('identity-content-encoding-classification.md','8f5886cabfc1fa5cc9494ab7974ba2274bb704feb986df36b550c98cd6c62c9f','Full1-84 original decision.')],
'No ARCH348 body observation witness or extra decompression/response/lifecycle owner. Both fixture trees full/local route1-43/README1-12 and actual semantic units1-46,main2824-2840,both registration/shared client gzip/deadline inspected. Support scopes corrected by actual EOF counts; current hashes unchanged. Original rules personally retained/revalidated; no execution/performance/mutations, supports zero.')
part('PART-490','/root/analyze_outbound',{
'header_cert_auth.rs':('ad783b3cf39e0ebc4368dd2772511d6b56b86fd55899ad1590c85fc48989f0ff','Full1-219 three real resolved-listener/mounted-certificate probes; positive identity/fingerprint remains mandatory, exact missing-cert status and source-header strip. No new cause/defaulting false pass.'),
'mod.rs':('cf3dafbccec67bd143fd103780edce59fc9b855143c6a1ab7610abdfeb2c3d62','Full1-10 private module/live export/scoped fixture contract; accepted.')},[
('chain-verification-header-mode.md','0a951b99d16bcead979cd988aae1ab79521c57ff20ce8f457fea96bcfb1cb762','Full original Header trust/CA provenance.'),
('header_cert_auth/plugin.rs','4f8876e7d1762dfec0c0dd45204e66e0fdae144dbd983bf89ebc2dadeb5f5139','Full1-338 live CA/decode/identity/hide/deny.'),
('header_cert_auth/plugin_test.rs','f381c795a67c19e50c21ed5d7518dc99d273a0d68fadc91a5d7916fd9799d20a','Full1-459 real crypto/identity/response/CA units.'),
('HeaderCertAuth/EdgionPlugins_default_header-cert-auth.yaml','a5504725df00daaf3537579baa023bf71c45880a87bf257fe471d6c20318589b','Full local/K8s active Header fixtures.'),
('gen_certs/generate_mtls_certs.sh','4488a01be91b13eb6e132f24b13b01f79c2fd1dd12f7f8ac06dbb6fea1054c93','155-184 real leaf Secret;229-257 CA producer.'),
('common/generate_runtime_certs.sh','6d3d2392798b7ac4686f90262f4412e105a818c0c23c375e4516a8b2bab134cb','360-380 CA;425-436 same leaf Secret producer.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','2654-2671 actual suite registration.')],
'Personally original rules/graded/index/routing/shared delay/Header originals read, no nested AGENTS. Both full fixture trees and real load/apply template-skip paths inspected. ARCH034 leaf DER and ARCH070 header-name admission existing roots; no new duplicate. Static only/no execution/builds/tests/network/mutation/live TLS proof or expanded chain construction; supports zero.')
s=r.state();p=r.RUN/s['candidates']['ARCH-354']['path'];assert r.digest(p)=='709b743ef3f0be54ad17473401abdc2153ee47f1899c647d51345e52bb1bc832'
body=p.read_text().split('---',2)[2];assert body.count('pg_upstream_peer::external_peer')==1
body=body.replace('pg_upstream_peer::external_peer','pg_upstream_peer::try_external_jump_peer')
assert hashlib.sha256(body.encode()).hexdigest()=='84242fb22207880511bbddd2974e98385422e6ef15763c5d1e6930a340a0e4f8'
r.confirm('ARCH-354','/root/verify_ip_union','709b743ef3f0be54ad17473401abdc2153ee47f1899c647d51345e52bb1bc832','1fd5eea569ab028d0c06ecaf1814b1b2357783cb6e8a1f813ee798f2e28bd4f5',[
'Independently confirmed unrelated 500/502 false attribution; exact approved symbol correction only.',
'Selection precedes admission, canonical BackendLocalhostNotAllowed records precise decision; shared K8s loopback override legitimate and must remain.',
'No production bypass/incident/execution; require isolated suite deny policy and correlated terminal evidence.'],supports([
('dynamic_external_upstream/dynamic_external_upstream.rs','7066f9b6db602730f29772d5a173a904ec583df114be9de6592549c778393992','Both rejection/fallback/log correlation cases and registrations.'),
('proxy_http/pg_upstream_peer.rs','1e78b4bf440134f3f8949ab883bc1f767c0e70055164818c35fc0e2251b25eaf','1538-1584 external jump;1678-1690 validation;1692-1709 rejected attempt.'),
('dynamic_external_upstream/plugin.rs','5e4f071c747f51bc9b7a6030cca25251aa6ece8b05fa8b82249f4574e4f12629','Full selection/jump/Host/dye/upstream-selected before admission.'),
('dynamic_external_upstream/plugin_test.rs','3d40322d10b73e5f3239c22ab4c236da5d57c141cde872c44d78b051840d10b9','Full real selection/Host/dye/rejected-jump/fallback units.'),
('integration/conf/base/EdgionGatewayConfig.yaml','0e024b70c3393ed8fb2d8672e03ac1d242caa933daaf0b36b6e102903d1e4258','Full local base loopback override omitted.'),
('k8s/conf/base/EdgionGatewayConfig.yaml','f93e62d71514f480822b84edd5705890ca826c9b07e995bc7b5fdab1fed01566','Full shared base explicit allow-loopback true.'),
('upstream-address-safety.md','09bc36a6ebf56e5706f29552cb9f1236ab661e0cea88380fc5608e8eb6c97e90','Full original admission/authorization/override/Endpoint exemption.'),
('client/access_log_client.rs','f7691b8ad4c8765a0ab446c414a1596df48c930d8472678d724f918edcf84d55','Canonical marker/traceparent/authenticated retries/exact error diagnostics.')]),replacement_body=body)
checkpoint()
# F changes task acceptance records only; preserve independently verified source closure.
s=r.state();a=json.loads((r.RUN/'BASELINE-AUDIT-20261011-E.json').read_text());b=json.loads((r.RUN/'BASELINE-AUDIT-20261011-F.json').read_text());assert a['files']==b['files']
changes={p for p in a['support'] if p in b['support'] and a['support'][p]!=b['support'][p]}
assert all(p.startswith('tasks/pending/architecture-plugin-session-ai-capability/') for p in changes)
affected=[]
for i,part in s['partitions'].items():
 effective={}
 for f in part.get('reports',[]):
  if (r.RUN/f).exists():
   d=json.loads((r.RUN/f).read_text());effective.update({p:v.get('sha256') if isinstance(v,dict) else v for p,v in d.get('support_selected',{}).items()});effective.update(d.get('current_support',{}))
 if set(effective)&changes:affected.append(i)
assert not affected,affected
(r.RUN/'DRIFT-DEPENDENCIES-20261011-F.json').write_text(json.dumps({'changed_sources':[],'changed_task_paths':sorted(changes),'added_support':sorted(set(b['support'])-set(a['support'])),'affected_primary_partitions':[], 'resolution':'No source drift. Personally read current independent AI result review: source approved, overall acceptance still blocked by unchanged required context-size failure. Preserve ARCH001 source-level closure; do not claim overall implementation acceptance or fresh executed checks.'},indent=2)+'\n')
s.update(current_baseline='BASELINE-AUDIT-20261011-F.json',current_fingerprint=b['fingerprint'],current_head=b['head']);s['resume_dispatch']={'analysis_conversion':['PART-493','PART-494','PART-495'],'analysis_outbound':['PART-491','PART-492'],'independent_verifier':'ARCH-355'};r.save(s);checkpoint()
