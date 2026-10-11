"""Coordinator-only atomic review artifact bookkeeping; no reviewed-file writes."""
import hashlib
import json
from pathlib import Path

RUN = Path(__file__).resolve().parent.parent
REPO = RUN.parent.parent

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def state():
    return json.loads((RUN / 'STATE.json').read_text())

def save(value):
    temporary = RUN / 'STATE.json.tmp'
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(RUN / 'STATE.json')

def candidate(ident, slug, partition, analyst, kind, priority, body):
    value = state()
    assert ident not in value['candidates']
    filename = ident + '-' + slug + '.md'
    content = ('---\nid: ' + ident + '\nstatus: candidate\nkind: ' + kind
               + '\npriority: ' + priority + '\npartition: ' + partition
               + '\nanalyst: ' + analyst + '\nbaseline: '
               + value['current_fingerprint'] + '\n---\n' + body)
    path = RUN / filename
    assert not path.exists()
    path.write_text(content)
    value['candidates'][ident] = dict(path=filename, status='candidate',
                                    partition=partition, sha256=digest(path),
                                    verification=None, kind=kind, analyst=analyst)
    save(value)
    return value['candidates'][ident]

def partition(ident, analyst, dispositions, supports, candidates, limits):
    value = state()
    part = value['partitions'][ident]
    assert part['status'] != 'reviewed'
    assert set(dispositions) == set(part['paths'])
    entries = []
    for path in part['paths']:
        sha = digest(REPO / path)
        assert sha == value['files'][path]['sha256'], path
        entries.append(dict(path=path, sha256=sha, fully_read=True,
                            disposition=dispositions[path]))
    evidence = {}
    for path, scope in supports.items():
        evidence[path] = dict(sha256=digest(REPO / path), scope=scope)
    filename = 'REPORT-' + ident + '.json'
    report = dict(partition=ident, analyst=analyst, complete=True, primary=entries,
                  candidates=candidates, support_selected=evidence, limits=limits)
    (RUN / filename).write_text(json.dumps(report, indent=2) + '\n')
    part.update(status='reviewed', terminal_complete=True, reports=[filename])
    for entry in entries:
        value['files'][entry['path']].update(status='reviewed', reviewer=analyst,
                                           disposition=entry['disposition'], evidence=filename)
    save(value)

def confirm(ident, verifier, input_hash, approved_claim, evidence, supports,
            replacement_body=None):
    value = state()
    entry = value['candidates'][ident]
    path = RUN / entry['path']
    assert digest(path) == input_hash
    content = path.read_text()
    if replacement_body is not None:
        sections = content.split('---', 2)
        content = '---' + sections[1] + '---' + replacement_body
    sections = content.split('---', 2)
    fields = [line for line in sections[1].splitlines()
              if line.startswith(('kind:', 'priority:'))]
    claim = hashlib.sha256(('\n'.join(fields) + '\n' + sections[2]).encode()).hexdigest()
    assert claim == approved_claim, (ident, claim)
    filename = 'VERIFICATION-' + ident + '.json'
    receipt = dict(candidate=ident, complete=True, verifier=verifier,
                   decision='confirmed-with-revision' if replacement_body else 'confirmed',
                   input_sha256=input_hash, approved_claim_sha256=approved_claim,
                   evidence=evidence, primary_coverage=0,
                   support_selected={p: dict(sha256=digest(REPO / p), scope=scope)
                                     for p, scope in supports.items()},
                   limits='Static independent review; no builds/tests/network/mutations; external consumers unaudited.')
    (RUN / filename).write_text(json.dumps(receipt, indent=2) + '\n')
    sections = content.split('---', 2)
    sections[1] = sections[1].replace('status: candidate', 'status: confirmed')
    path.write_text('---' + sections[1] + '---' + sections[2])
    entry.update(status='confirmed', sha256=digest(path),
                 verification=dict(complete=True, verifier=verifier,
                                   input_sha256=input_hash,
                                   approved_claim_sha256=approved_claim,
                                   evidence=filename))
    save(value)
