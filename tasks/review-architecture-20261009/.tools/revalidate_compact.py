"""Preserve historical primary receipts and append complete bounded revalidation."""
import json,subprocess
import record as r
GATES={
'AGENTS.md':'cba9f532fa1c9f70e6a921e1a2331e7ca78c8210981a072a74cdebf4d85d3e42',
'skills/SKILL.md':'f0dfcd1ba9ddb16e1042dcadb9edcb1b9a33ad1e146b9ed45eb80522375f32a6',
'skills/00-design-rules.md':'c78c86675ca6c72adb30d0f59727509486b582ef2c7f3318bf14e61745eff055',
'skills/04-review/knowledge-routing.md':'b4e5658c454ae3378d1ff71722cecf46e07543329eca849766ee7e9676634a08',
}
def restore(ident,reviewer,primaries,rows,limits,preserved_findings=()):
 s=r.state();part=s['partitions'][ident];assert part['status']=='needs-revalidation'
 primary=[]
 for p in part['paths']:
  sha,scope,disposition=primaries[p.rsplit('/',1)[1]];assert sha==r.digest(r.REPO/p)==s['files'][p]['sha256'],p
  primary.append({'path':p,'sha256':sha,'fully_read':True,'scope':scope,'disposition':disposition})
 assert len(primary)==len(primaries)
 paths=subprocess.check_output(['rg','--files','edgion-gateway','edgion-controller','edgion-resources','edgion-common','edgion-tests','skills','tasks/review-architecture-20261009'],cwd=r.REPO,text=True).splitlines()
 supports={}
 for hint,sha,scope in rows:
  matches=[p for p in paths if hint in p and r.digest(r.REPO/p)==sha];assert matches,(hint,sha)
  for p in matches:supports[p]={'sha256':sha,'scope':scope}
 for p,h in GATES.items():assert r.digest(r.REPO/p)==h,p
 effective={}
 for f in part['reports']:
  d=json.loads((r.RUN/f).read_text());effective.update({p:v.get('sha256') if isinstance(v,dict) else v for p,v in d.get('support_selected',{}).items()});effective.update(d.get('current_support',{}))
 drift={p for p,h in effective.items() if not (r.REPO/p).exists() or r.digest(r.REPO/p)!=h}
 assert drift.issubset(set(supports)|set(GATES)),sorted(drift-set(supports)-set(GATES))
 name='REVALIDATION-'+ident+'-20261011-H.json';assert not (r.RUN/name).exists()
 receipt={'partition':ident,'reviewer':reviewer,'complete':True,'baseline':s['current_baseline'],'new_primary_coverage':0,'primary':primary,'support_selected':supports,'current_support':GATES|{p:e['sha256'] for p,e in supports.items()},'preserved_findings':list(preserved_findings),'limits':limits+' Historical receipt retained; unchanged primary coverage restored only after actual current context inspection. Findings are not independently reapproved by the analyst.'}
 (r.RUN/name).write_text(json.dumps(receipt,indent=2)+'\n')
 part.update(status='reviewed',terminal_complete=True,revalidated_baseline=s['current_baseline']);part['reports'].append(name)
 for p in part['paths']:s['files'][p].update(status='reviewed',revalidated_baseline=s['current_baseline'],revalidated_by=reviewer)
 r.save(s);subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
