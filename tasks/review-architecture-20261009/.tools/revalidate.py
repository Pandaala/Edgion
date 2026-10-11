"""Coordinator-only scoped evidence reconciliation; no reviewed-file writes."""
import json
import record as r

def restore(ident, reviewer, supports, conclusion):
    state = r.state()
    part = state['partitions'][ident]
    assert part['status'] == 'needs-revalidation'
    drift = json.loads((r.RUN / 'DRIFT-DEPENDENCIES-20261011-B.json').read_text())['partitions'][ident]
    assert not drift['changed_primary']
    assert set(drift['support_drift']).issubset(supports)
    for rel in part['paths']:
        entry = state['files'][rel]
        assert r.digest(r.REPO / rel) == entry['sha256'] == entry['previous_reviewed_sha256']
    name = 'REVALIDATION-' + ident + '-20261011-B.json'
    receipt = dict(partition=ident, reviewer=reviewer, complete=True,
                   baseline=state['current_baseline'], new_primary_coverage=0,
                   conclusion=conclusion,
                   current_support={p:r.digest(r.REPO/p) for p in supports},
                   support_selected={p:dict(sha256=r.digest(r.REPO/p),scope=v) for p,v in supports.items()},
                   limits='Restore historic unchanged primary reads after actual current supporting revalidation. No new primary reading, candidate self-verification or executed tests.')
    (r.RUN/name).write_text(json.dumps(receipt,indent=2)+'\n')
    part.update(status='reviewed',terminal_complete=True,revalidated_baseline=state['current_baseline'])
    part['reports'].append(name)
    for rel in part['paths']:
        state['files'][rel].update(status='reviewed',revalidated_baseline=state['current_baseline'])
    r.save(state)

def preserve_verification(ident):
    state = r.state()
    candidate = state['candidates'][ident]
    if candidate.get('verification'):
        candidate.setdefault('verification_history',[]).append(candidate['verification'])
    receipt = r.RUN/('VERIFICATION-'+ident+'.json')
    if receipt.exists():
        saved = r.RUN/('VERIFICATION-'+ident+'-PRE-20261011-B.json')
        assert not saved.exists()
        saved.write_bytes(receipt.read_bytes())
    r.save(state)
