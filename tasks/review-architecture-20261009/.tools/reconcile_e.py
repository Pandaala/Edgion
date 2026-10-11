"""Reconcile newly detected AI capability edits without discarding unrelated work."""
import json
import record as r

state=r.state()
before=json.loads((r.RUN/'BASELINE-AUDIT-20261011-D.json').read_text())
after=json.loads((r.RUN/'BASELINE-AUDIT-20261011-E.json').read_text())
changed={p for p in before['files'] if p in after['files'] and before['files'][p]!=after['files'][p]}
changed |= {p for p in before['support'] if p in after['support'] and before['support'][p]!=after['support'][p]}
added=set(after['files'])-set(state['files'])
assert not (set(state['files'])-set(after['files']))
(r.RUN/'STATE-PRE-20261011-E.json').write_text(json.dumps(state,indent=2)+'\n')
def selected(reports):
    evidence={}
    for filename in reports:
        path=r.RUN/filename
        if not path.exists():
            continue
        data=json.loads(path.read_text())
        for p,entry in data.get('support_selected',{}).items():
            evidence[p]=entry.get('sha256') if isinstance(entry,dict) else entry
        evidence.update(data.get('current_support',{}))
    return evidence

affected={}
for ident,part in state['partitions'].items():
    primary=[p for p in part['paths'] if p in changed]
    evidence=selected(part.get('reports',[]))
    drift={p:{'reviewed_sha256':sha,'current_sha256':r.digest(r.REPO/p)} for p,sha in evidence.items()
           if p in changed and sha!=r.digest(r.REPO/p)}
    if primary or drift:
        affected[ident]=dict(changed_primary=primary,support_drift=drift)
        if part['status']=='reviewed':
            part.update(status='needs-revalidation', terminal_complete=False, revalidation_reason='DRIFT-DEPENDENCIES-20261011-E.json')
            for p in part['paths']:
                entry=state['files'][p]
                entry.update(previous_reviewed_sha256=entry['sha256'],status='needs-revalidation',revalidation_reason='DRIFT-DEPENDENCIES-20261011-E.json')
for p in state['files']:
    state['files'][p].update(after['files'][p])
for p in added:
    ident='GW-AI-CAPABILITY'
    assert ident not in state['partitions']
    state['partitions'][ident]=dict(crate='edgion-gateway',owner='edgion-gateway/src/plugins/runtime/traits/ai_session.rs',paths=[p],status='pending',bytes=after['files'][p]['bytes'],reports=[])
    state['files'][p]=after['files'][p] | dict(partition=ident,status='pending')
candidate_drift={}
for ident,entry in state['candidates'].items():
    verification=entry.get('verification') or {}
    evidence=verification.get('evidence')
    selection={}
    if isinstance(evidence,str) and (r.RUN/evidence).exists():
        data=json.loads((r.RUN/evidence).read_text())
        selection={p:v.get('sha256') if isinstance(v,dict) else v for p,v in data.get('support_selected',{}).items()}
    elif isinstance(evidence,dict):
        selection=evidence
    drift={p:{'reviewed_sha256':sha,'current_sha256':r.digest(r.REPO/p)} for p,sha in selection.items()
           if p in changed and sha!=r.digest(r.REPO/p)}
    if drift:
        candidate_drift[ident]=drift
        if entry['status']=='confirmed':
            path=r.RUN/entry['path']
            path.write_text(path.read_text().replace('status: confirmed','status: candidate',1))
            entry.update(previous_status='confirmed',status='candidate',sha256=r.digest(path),revalidation_pending=True,revalidation_reason='DRIFT-DEPENDENCIES-20261011-E.json')
state.update(current_head=after['head'],current_fingerprint=after['fingerprint'],current_baseline='BASELINE-AUDIT-20261011-E.json')
r.save(state)
(r.RUN/'DRIFT-DEPENDENCIES-20261011-E.json').write_text(json.dumps(dict(baseline='BASELINE-AUDIT-20261011-E.json',scope='Current effective recorded dependencies; previous invalidations remain pending.',changed_paths=sorted(changed),added_primary=sorted(added),partitions=affected,candidates=candidate_drift),indent=2)+'\n')
print(json.dumps(dict(current_files=len(state['files']),affected_partitions=len(affected),affected_candidates=len(candidate_drift),current_fingerprint=after['fingerprint'])))
