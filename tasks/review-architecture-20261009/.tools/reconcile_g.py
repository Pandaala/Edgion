"""Reconcile supporting layout assertions and record external final AI acceptance."""
import json
import record as r
s=r.state();a=json.loads((r.RUN/s['current_baseline']).read_text());b=json.loads((r.RUN/'BASELINE-AUDIT-20261011-G.json').read_text());assert a['files']==b['files']
changed={p for p in a['support'] if p in b['support'] and a['support'][p]!=b['support'][p]}
(r.RUN/'STATE-PRE-20261011-G.json').write_text(json.dumps(s,indent=2)+'\n')
affected={}
for i,part in s['partitions'].items():
 selection={}
 for f in part.get('reports',[]):
  if (r.RUN/f).exists():
   d=json.loads((r.RUN/f).read_text());selection.update({p:v.get('sha256') if isinstance(v,dict) else v for p,v in d.get('support_selected',{}).items()});selection.update(d.get('current_support',{}))
 drift={p:{'recorded':h,'current':r.digest(r.REPO/p)} for p,h in selection.items() if p in changed and h!=r.digest(r.REPO/p)}
 if drift:
  affected[i]=drift
  if part['status']=='reviewed':
   part.update(status='needs-revalidation',terminal_complete=False,revalidation_reason='DRIFT-DEPENDENCIES-20261011-G.json')
   for p in part['paths']:s['files'][p].update(status='needs-revalidation',previous_reviewed_sha256=s['files'][p]['sha256'],revalidation_reason='DRIFT-DEPENDENCIES-20261011-G.json')
s.update(current_baseline='BASELINE-AUDIT-20261011-G.json',current_fingerprint=b['fingerprint'],current_head=b['head'])
(r.RUN/'DRIFT-DEPENDENCIES-20261011-G.json').write_text(json.dumps({'sources_changed':[],'support_changed':sorted(changed),'support_added':sorted(set(b['support'])-set(a['support'])),'partitions':affected,'limits':'Recorded effective dependency audit only; prior invalidations remain pending. Unrelated rule/index clarification revalidations retained.'},indent=2)+'\n')
paths=['tasks/pending/architecture-plugin-session-ai-capability/07-final-result-review.md','tasks/pending/architecture-plugin-session-ai-capability/06-context-budget-amendment.md','tasks/pending/architecture-plugin-session-ai-capability/amendment-lightweight-verification.json','skills/04-review/cpu-memory/intentional-design/http-request-context-layout-budget.md']
receipt={'finding':'ARCH-001','reviewer':'/root','baseline':'BASELINE-AUDIT-20261011-G.json','source_closure':'Preserve original independent source capability resolution; no production source changed versus F.','implementation_acceptance':'Existing separate AI task final independent reviewer reports PASS for combined capability extraction and approved layout assertion correction; previous required gates resolved.','reported_execution':'Existing task receipt reports31 amendment tests passed plus final workspace check/Clippy/format/guards. This read-only review did not execute those commands.','support_selected':{p:{'sha256':r.digest(r.REPO/p),'scope':'Personally read complete current task acceptance/amendment/guard record or original layout decision; stale proposal/failed receipts remain historical.'} for p in paths},'new_primary_coverage':0,'new_findings':0,'limits':'Acceptance status evidence from separate workflow, not our fresh product execution or broad implementation approval.'}
(r.RUN/'TASK-ACCEPTANCE-AI-20261011-G.json').write_text(json.dumps(receipt,indent=2)+'\n');s['candidates']['ARCH-001']['current_implementation_acceptance']='TASK-ACCEPTANCE-AI-20261011-G.json';r.save(s)
print('Source unchanged; affected recorded partitions',len(affected))
