"""Persist received complete read-only reports and independently bound revision."""
import subprocess, hashlib
import record as r
paths=subprocess.check_output(['rg','--files','edgion-gateway','edgion-resources','edgion-controller','edgion-tests','skills'],cwd=r.REPO,text=True).splitlines()
def checkpoint(): subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
def supports(rows):
 out={}
 for suffix,sha,scope in rows:
  matches=[p for p in paths if p.endswith(suffix) and r.digest(r.REPO/p)==sha]
  assert matches,suffix
  for p in matches:out[p]=scope
 return out
def part(i,analyst,primary,rows,limits):
 s=r.state();p=s['partitions'][i];d={}
 for path in p['paths']:
  sha,desc=primary[path.rsplit('/',1)[1]];assert r.digest(r.REPO/path)==sha,path;d[path]=desc
 r.partition(i,analyst,d,supports(rows),[],limits)
 s=r.state();s['partitions'][i]['bytes']=sum((r.REPO/p).stat().st_size for p in s['partitions'][i]['paths']);r.save(s);checkpoint()
part('PART-486','/root/analyze_outbound',{
'graphql_rate_limit.rs':('b528173c2bc40c889759f7af5c874b88e25bec915c642471478cc8fb9b80883e','Full1-852 disjoint complete reads; unique-key weighted debit, unchanged bytes, retained routing-failure debit, shared/isolated quota, concurrent Redis admission, GET/POST costs, parser/cold-invalid/disabled/dependency modes and root arithmetic agree with canonical owners. No new cause.'),
'mod.rs':('198335224dea349e0f5ef6eb319c16b07a38c620593fc35edb1a1ef96178a9a2','Full1-3 private implementation and two real exports/registration; accepted.')},[
('graphql-query-cost-budgets.md','eac2cd61092931fc5e6dac43fe348481bbd1c56941ba9be03bef95fbd41dfc7d','Full original completed cost/root capacity/debit/body/admission contract.'),
('redis-named-quotas.md','c2bb243e81b34bb761d1d72552f3a8ac3d57f58c6e29107cc1033e43ceb9b702','Full original independent named quota and partial-debit contract.'),
('graphql_rate_limit/plugin.rs','ae318d23b9c655a49820985bf6a64ecd99fcc9015e33360b1054d7783f25ef99','Full1-89 completed estimate/capacity/canonical limiter delegation.'),
('graphql_rate_limit/plugin_test.rs','6681b503878f71a8c067e076060f0c854a781803ccfc0f0d7112ee2f74a80dde','Full1-201 real constructor/admission/estimate/capacity unit bodies.'),
('scenarios/graphql_rate_limit.rs','a654c0815b7a692e6415216941e69ec6f47da7d639194420674830830dfe0e1a','Full1-118 eleven phase factories/updates.'),
('rate_limit_redis/plugin.rs','8300ea1ea1c66a0c513021699ae66e92ad5008d44485e923f401c07db360827c','123-194 preparation/sequential check;336-442 computed-cost/deadline/final renderer.'),
('GraphqlRateLimit/13_EdgionPlugins_root-quotas.yaml','685a702999961eb7277a4f79ab4a75f16a9f7c2abc3725d27ef48a7c85f40140','Full local custom root policies/route.'),
('GraphqlRateLimit/13_EdgionPlugins_root-quotas.yaml','5e36afa6511995dbed726943c648d7dd2fec67c1102c5fd403bcff3e704feba9','Full Kubernetes roots/in-cluster Redis identity.')],
'Original rules personally retained/fresh unchanged; no nested AGENTS. Both fixture trees static/dynamic full, actual registry3104-3124 and shared transport/status/echo read. Correct Controller status selected. ARCH264 restoration/current ARCH304 publication drift separate. No execution/builds/tests/network/mutations/performance or transactional guarantee; supports zero coverage.')
part('PART-487','/root/analyze_conversion',{
'mod.rs':('fec137e1320cb69d52c69afd1957b84254a42af7a99bd5fd8e651c391a9b5589','Full1-266 split1-150/151-266,9798bytes; twelve static cases and parameterized dynamic phase: fallible complete exact response/origin/echo and fixed rejection JSON/content-type/final logs; selected Controller status. No new cause.')},[
('scenarios/graphql_safelist.rs','6f68f3fd3e62905c5909d8e4bc14b3c8232e3e4ed42471a0398d7a3a72c41183','Full1-62 actual initial/invalid/recovered/revoked lifecycle leaves revoked generation.'),
('suites/status/helpers.rs','0236de331bc053f9ba00610543ef4e7a8932a9d49e129110f068091d98f823dc','1-144 selected status helper and real unit bodies.'),
('plugins/http/graphql_safelist/mod.rs','8c69925c5ef9f407c370f1662bc0fc9b3d742905356f1b9fcfe3cb7ad3e8cfa5','Full1-44 live adapter.'),
('graphql_safelist/mod_test.rs','db8e35dc328d0ffa1e75d4120b4a6512f44ce57550e90232c93617f3ca4c3fd6','Full1-59 real units.'),
('GraphqlSafelist/10_EdgionPlugins_graphql-safelist.yaml','d32924db9695409f78265cb50f242261fe14acea8c6aa3e7b487ef2a44b09916','Full1-29 both local/Kubernetes static policy.'),
('graphql-static-safelist.md','88673418784202ccec39fff317445fae0a7acbac3e72c0a5f61db581d82ed7ad','Full1-34 original contract.')],
'Same ARCH264 pending context supplement: scenario44-55 initial suite expects ping/pong200 but revoked return leaves pong disabled; main3742-3763 only context/invocation, no initial/reset owner. Bounded initial apply and result-aware restore preserve all business phases. Current local runner1992-1993 and K8s registration59 exist; historical ARCH304 absence not reusable. Other entire static/dynamic fixtures and READMEs, server810-839/main3742-3770/context162-205/registry3090-3105 actual owners personally inspected. Original rules fresh retained; no execution/mutation; supporting zero.')
part('PART-488','/root/analyze_conversion',{
'guardrail.rs':('c1ffe73418163f81af2052f88227b2731ef1526638e37ef90bc80a1b812767b7','Full1-803 sequential four ranges,35516bytes; eleven explicit cases: exact pass/replace/fail-open, canonical reject+origin403 H1/H2, bounded serial windows/timeout, ExtProc precedence, Anthropic translation/error bypass, aggregate log ownership; accepted.'),
'mod.rs':('21d11d4b80cfe128bc107d5386bfa0070d5f8d2431745989d68215a53dbc676a','Full1-3,55bytes actual export/registration; accepted.')},[
('response.rs','59f8e9a5e84c2357a79aa16cdb38888f0417126c01f430c6f01dcb89a1559d37','380-462 explicit discard/termination/policy owner.'),
('response_test.rs','2f0076a920467de2ca0888821f4fabfb8fd671cf49e457265180ac4b7fa41466','387-429,544-626 actual two-call/reject units.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','1490-1555,1696-1817,5127-5194 deterministic split/two windows/timeout/content-free unit.'),
('03_EdgionPlugins_guardrail.yaml','d22c3faebd547287ff267c44d32fccac25254b0f4c247c4e83082dab125f18dc','Full1-368 local live policy.'),
('Guardrail/resources.yaml','f7222e08ef8a649803ff7fd178b4ea42fa3bea487e9f5e01c8191864eed0538d','Full1-610 Kubernetes live resources.'),
('window-emitted-context.md','afd9738fbed99f1e38477ad8cf0f147f87a744d0b0de3ff2200bd562016ed4d5','Full original decision.'),
('02-gateway/00-overview.md','042a429644dc4be2a1fa31817cb6ab27dbcbe61d27cca72e7a37b7a9e84d9def','87-124 response-window/body-boundary contract.')],
'No new candidate: defaulted bodies cannot satisfy required nonempty/exact markers; timeout loop intentionally permits termination but separately requires real one-call fixture. No ARCH348 extension. Current K8s base AiGateway owns older reference legitimately; held response uses SingleProvider backend. Full both fixture trees/main2824-2840/local+K8s runners/base/backend/apply owner read. Original rules fresh retained, no nested AGENTS. Static only/no runtime/performance/execution/mutations; supporting zero.')
s=r.state();q=s.setdefault('pending_supplements',{}).setdefault('ARCH-264',[])
if 'REPORT-PART-487.json' not in q:q.append('REPORT-PART-487.json')
s['resume_dispatch']={'analysis_conversion':['PART-489'],'analysis_outbound':['PART-490','PART-491','PART-492'],'independent_verifier':'ARCH-354'};r.save(s)
s=r.state();candidate=r.RUN/s['candidates']['ARCH-353']['path'];assert r.digest(candidate)=='98cc3ef3938781bc1df888d6cdce9b101ec7d02127b566dfde25cb3714cd1e34'
body=candidate.read_text().split('---',2)[2]
old='The preceding store-status case does not repair the boundary: cases can be selected independently, and infrastructure can fail after that check.'
new='The preceding store-status case does not repair the boundary: infrastructure can fail after that check, and it does not witness this request or its correlated terminal record.'
assert body.count(old)==1;body=body.replace(old,new)
assert hashlib.sha256(body.encode()).hexdigest()=='da2d73048547e3d57e7eb8f92165f3f9878ae52e17fda0d93cd270d77ab27f25'
sp=supports([
('debug_access_log/logs.rs','2c83844e7ccc9e1c101d762e7810da3693020871cca9eec761cd93dfe2275036','Model19-62,helper67-99,empty263-299,status359-404,registration411-430.'),
('client/access_log_client.rs','f7691b8ad4c8765a0ab446c414a1596df48c930d8472678d724f918edcf84d55','Full marker/traceparent/auth/HTTP/envelope/JSON/retries and real unit.'),
('proxy_http/pg_logging.rs','972f72ec7371c3ab876bfa12957eb70559e31aaa9d25f8f2078c24e197523a6a','1-145 canonical producer;104-144 opt-in storage has no plugin-presence gate.'),
('observe/access_log/entry.rs','a0db01fd33530ef61dc5bd4f356320b0bdfd940111273b0f8ac260e6ab518965','174-178 stage omission;230-257 context projection.'),
('observe/access_log_store.rs','db0f8065a6dfc62de5f0050c6f262081cd9625c3022c00add03df4fe5cd735b7','1-220 bounded admission/TTL/status.'),
('edgion-gateway/src/api/mod.rs','a7b6ba1fd44e1b5f7dad36c738e970bbf7b31e95b2cd5de73820904271de9662','276-306 testing/auth mount;334-350 envelope;646-664 endpoints.'),
('observe/access_log/entry_test.rs','21b2b3b5b6486bdab12b6fb831b2d94a944fbcd7de6dc20571b0da15c5118caa','227-239 actual absent-state omission unit.'),
('observe/access_log_store_test.rs','0ecf063cfe5aea4e707ea4c320ff3eed14bb128788d93ddcca30bbcd67196da5','1-19,100-151 real store/missing/admission/TTL units.')])
r.confirm('ARCH-353','/root/verify_ip_union','98cc3ef3938781bc1df888d6cdce9b101ec7d02127b566dfde25cb3714cd1e34','05ab0058d6787b01a4250dbec238f5bff56e27744a0ba3e5343d0f65c9276a9d',[
'Independent verifier confirms observation-error false PASS; current producer stores opt-in terminal records even with no plugins.',
'Exact approved revision removes unsupported independent single-case selection claim; serial suite selection does not witness later request.',
'Original rules/topic decisions/both fixture trees and real units read personally; no execution or incident claim.'],sp,replacement_body=body)
checkpoint()
