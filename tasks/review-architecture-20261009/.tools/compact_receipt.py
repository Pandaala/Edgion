"""Serial coordinator sink for explicit, hash-bound completed analyst receipts."""
import json,subprocess
import record as r

def persist(ident,analyst,primaries,support_rows,limits,candidate_ids=()):
    state=r.state();part=state['partitions'][ident]
    assert part['status']!='reviewed',ident
    d={}
    for p in part['paths']:
        sha,scope,disposition=primaries[p.rsplit('/',1)[1]]
        assert sha==r.digest(r.REPO/p)==state['files'][p]['sha256'],p
        d[p]=scope+'; '+disposition
    assert len(d)==len(primaries)
    paths=subprocess.check_output(['rg','--files','edgion-gateway','edgion-resources','edgion-controller','edgion-common','edgion-algo','edgion-tests','skills'],cwd=r.REPO,text=True).splitlines()
    evidence={}
    for hint,sha,scope in support_rows:
        matches=[p for p in paths if hint in p and r.digest(r.REPO/p)==sha]
        assert matches,(hint,sha)
        for p in matches:evidence[p]=scope
    r.partition(ident,analyst,d,evidence,list(candidate_ids),limits)
    state=r.state();state['partitions'][ident]['bytes']=sum((r.REPO/p).stat().st_size for p in part['paths']);r.save(state)
    subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
