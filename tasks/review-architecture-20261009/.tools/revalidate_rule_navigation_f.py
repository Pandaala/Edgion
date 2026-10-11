"""Bound rule-only dependency drift by the personally inspected exact Git diff."""
import record as r
import json,subprocess,hashlib
s=r.state();audit=json.loads((r.RUN/'AUDIT-CURRENT-REVIEWED-20261011-F.json').read_text());base='a85aed2fbb73df274dea2932eef8b1be2f9152a2'
allowed={'AGENTS.md','skills/00-design-rules.md','skills/04-review/waf/SKILL.md','skills/04-review/cpu-memory/SKILL.md'}
diff=subprocess.check_output(['git','diff',base,'--',*sorted(allowed)],cwd=r.REPO,text=True)
(r.RUN/'RULE-NAVIGATION-DIFF-20261011-F.txt').write_text(diff)
for i,v in audit['issues'].items():
 assert not v['primary'] and set(v['support'])<=allowed,i
 supports={}
 for p,h in v['support'].items():
  old=subprocess.check_output(['git','show',base+':'+p],cwd=r.REPO)
  assert hashlib.sha256(old).hexdigest()==h['recorded'],p
  assert r.digest(r.REPO/p)==h['current'],p
  scope={'AGENTS.md':'Complete exact diff and current cross-cutting row: canonical resolver obligation unchanged, obsolete single function spelling replaced by namespace.',
   'skills/00-design-rules.md':'Complete exact diff and current canonical row: same mandatory TLS resolver obligation, no additional or relaxed gate.',
   'skills/04-review/waf/SKILL.md':'Full current index and complete exact diff: missing historical optimization task path is now explicitly historical; no regression guard or delivered-WAF contract changes.',
   'skills/04-review/cpu-memory/SKILL.md':'Full index/exact diff and full newly linked HTTP context-layout page: extra Gateway-only layout navigation does not change Controller observe primary contracts in PART429.'}[p]
  supports[p]={'sha256':h['current'],'scope':scope}
 part=s['partitions'][i]
 for p in part['paths']:assert r.digest(r.REPO/p)==s['files'][p]['sha256'],p
 filename='REVALIDATION-'+i+'-RULE-NAVIGATION-20261011-F.json'
 receipt={'partition':i,'complete':True,'reviewer':'/root','baseline':'BASELINE-AUDIT-20261011-F.json','new_primary_coverage':0,'support_selected':supports,'conclusion':'Original full primary review retained; exact changed dependencies only clarify canonical TLS naming and index navigation. Actual source dependencies remain unchanged. No substantive claim revision, new primary reading credit, independent approval replacement or acceptance of new layout task.','diff':'RULE-NAVIGATION-DIFF-20261011-F.txt','diff_sha256':r.digest(r.RUN/'RULE-NAVIGATION-DIFF-20261011-F.txt'),'limits':'Coordinator scoped read-only rule/navigation revalidation; no tests/builds/execution or changes outside result directory.'}
 (r.RUN/filename).write_text(json.dumps(receipt,indent=2)+'\n');part['reports'].append(filename)
r.save(s)
print('Recorded',len(audit['issues']),'bounded unchanged-primary rule/navigation revalidations; zero new primary coverage.')
