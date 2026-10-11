import json,subprocess
import record as r
s=r.state();a=json.loads((r.RUN/s['current_baseline']).read_text());b=json.loads((r.RUN/'BASELINE-AUDIT-20261011-H.json').read_text());assert a['files']==b['files']
removed=set(a['support'])-set(b['support']);changed={p for p in a['support'] if p in b['support'] and a['support'][p]!=b['support'][p]};impact=removed|changed
(r.RUN/'STATE-PRE-20261011-H.json').write_text(json.dumps(s,indent=2)+'\n')
affected={};candidates={}
for k,part in s['partitions'].items():
 effective={}
 for f in part.get('reports',[]):
  d=json.loads((r.RUN/f).read_text());effective.update({p:v.get('sha256') if isinstance(v,dict) else v for p,v in d.get('support_selected',{}).items()});effective.update(d.get('current_support',{}))
 drift={p:{'recorded':h,'current':r.digest(r.REPO/p) if (r.REPO/p).exists() else None} for p,h in effective.items() if p in impact}
 if drift:
  affected[k]=drift
  if part['status']=='reviewed':
   part.update(status='needs-revalidation',terminal_complete=False,revalidation_reason='DRIFT-DEPENDENCIES-20261011-H.json')
   for p in part['paths']:s['files'][p].update(status='needs-revalidation',previous_reviewed_sha256=s['files'][p]['sha256'],revalidation_reason='DRIFT-DEPENDENCIES-20261011-H.json')
for k,c in s['candidates'].items():
 v=c.get('verification') or {};f=v.get('evidence');d=json.loads((r.RUN/f).read_text()) if isinstance(f,str) and (r.RUN/f).exists() else {};drift={p:e for p,e in d.get('support_selected',{}).items() if p in impact}
 if drift:
  candidates[k]=drift
  if c['status']=='confirmed':
   path=r.RUN/c['path'];path.write_text(path.read_text().replace('status: confirmed','status: candidate',1));c.update(previous_status='confirmed',status='candidate',sha256=r.digest(path),revalidation_pending=True,revalidation_reason='DRIFT-DEPENDENCIES-20261011-H.json')
s.update(current_baseline='BASELINE-AUDIT-20261011-H.json',current_fingerprint=b['fingerprint'],current_head=b['head'])
s['support_followups'].append({'id':'external_task_records_removed_20261011_H','removed':sorted(removed),'historical_acceptance':'TASK-ACCEPTANCE-AI-20261011-G.json remains a dated acceptance receipt. Its removed source task records are historical, not current live links. Preserve externally closed ARCH001; no source drift.','new_pending_task':'ARCH002 separate value assessment only; no implementation/source changes. Personally read original new task; no closure or automatic approval inferred.'})
r.save(s)
(r.RUN/'DRIFT-DEPENDENCIES-20261011-H.json').write_text(json.dumps({'sources_changed':[],'support_changed':sorted(changed),'support_removed':sorted(removed),'support_added':sorted(set(b['support'])-set(a['support'])),'partitions':affected,'candidates':candidates,'limits':'Source content unchanged across external commit; removed task dependencies require bounded revalidation. Historical evidence is preserved, without recreating external deletions or treating separate assessment as implementation.'},indent=2)+'\n')
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
