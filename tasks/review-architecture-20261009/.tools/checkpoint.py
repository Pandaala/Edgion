#!/usr/bin/env python3
"""Reconcile review state and render durable coverage/index; coordinator use only."""
import collections
import hashlib
import json
from pathlib import Path

RUN = Path(__file__).resolve().parent.parent


def claim_hash(content):
    sections = content.split('---', 2)
    body = sections[2] if len(sections) == 3 else content
    front = sections[1] if len(sections) == 3 else ''
    fields = [line for line in front.splitlines() if line.startswith(('kind:', 'priority:'))]
    return hashlib.sha256(('\n'.join(fields) + '\n' + body).encode()).hexdigest()


state_path = RUN / 'STATE.json'
state = json.loads(state_path.read_text())
owned = [p for part in state['partitions'].values() for p in part['paths']]
assert len(owned) == len(set(owned)) == len(state['files'])
assert set(owned) == set(state['files'])
for ident, part in state['partitions'].items():
    for rel in part['paths']:
        assert state['files'][rel]['partition'] == ident
    if part['status'] == 'reviewed':
        assert part.get('terminal_complete') is True
        assert all(state['files'][p]['status'] == 'reviewed' for p in part['paths'])
for ident, candidate in state['candidates'].items():
    path = (RUN / candidate['path']).resolve()
    assert path.parent == RUN
    if candidate.get('finding_removed'):
        assert candidate['status'] == 'resolved'
        assert not path.exists()
        closure = candidate['closure']
        assert closure['disposition'] == 'fixed-and-verified'
        assert (RUN / closure['acceptance_evidence']).is_file()
        assert candidate['verification']['complete'] is True
        continue
    assert path.is_file()
    assert hashlib.sha256(path.read_bytes()).hexdigest() == candidate['sha256']
    if candidate['status'] == 'confirmed':
        assert candidate['verification'] and candidate['verification'].get('complete') is True
        assert candidate['verification']['approved_claim_sha256'] == claim_hash(path.read_text())
counts = collections.defaultdict(collections.Counter)
for entry in state['files'].values():
    counts[entry['crate']][entry['status']] += 1
reviewed = sum(c['reviewed'] for c in counts.values())
lines = ['# Source coverage', '',
         'Primary coverage requires implementation and relevant call-chain inspection.',
         'Secondary evidence reads are not counted as primary coverage.', '',
         'Reviewed: %d / %d. Active crate: `%s`.' % (reviewed, len(owned), state['active_crate']), '',
         '| Crate | Total | Reviewed | Remaining |', '| --- | ---: | ---: | ---: |']
for crate, count in sorted(counts.items()):
    total = sum(count.values())
    lines.append('| %s | %d | %d | %d |' % (crate, total, count['reviewed'], total-count['reviewed']))
lines.extend(['', '## Per-file ledger', '', '| Path | SHA-256 | Partition | Status | Reviewer |',
              '| --- | --- | --- | --- | --- |'])
for rel, entry in sorted(state['files'].items()):
    lines.append('| `%s` | `%s` | %s | %s | %s |' % (rel, entry['sha256'],
                 entry['partition'], entry['status'], entry.get('reviewer') or 'pending'))
(RUN / 'COVERAGE.md').write_text('\n'.join(lines)+'\n')
lines = ['# Architecture optimization review', '',
         'Status: in progress. Production source is read-only.', '',
         'Workflow: `edgion-architecture-review` (personal skill).',
         'See [plan](PLAN.md), [coverage](COVERAGE.md), [state](STATE.json) and [baseline](BASELINE.json).', '',
         'Source reviewed: %d / %d; %d primary partitions.' % (reviewed,len(owned),len(state['partitions'])), '',
         '| ID | Status | Kind | Optimization |', '| --- | --- | --- | --- |']
for ident, candidate in sorted(state['candidates'].items()):
    if candidate.get('finding_removed'):
        lines.append('| %s | resolved | %s | [%s](%s) |' % (
            ident, candidate.get('kind', 'architecture-debt'),
            candidate['closure'].get('index_title', 'Closed feature-operation coupling finding; acceptance evidence'),
            candidate['closure']['acceptance_evidence']))
        continue
    content = (RUN/candidate['path']).read_text()
    title = next((line[2:] for line in content.splitlines() if line.startswith('# ')), None)
    if title is None:
        title = next(line.removeprefix('Title: ') for line in content.splitlines() if line.startswith('Title: '))
    lines.append('| %s | %s | %s | [%s](%s) |' % (ident,candidate['status'],
                 candidate.get('kind','architecture-debt'),title,candidate['path']))
lines.extend(['', '## Current limitations', '',
              'Pending files remain in scope. Recommendations await independent verification unless',
              'explicitly marked confirmed. No builds, tests, implementation or commits are part of this run.'])
(RUN/'INDEX.md').write_text('\n'.join(lines)+'\n')
print(json.dumps({'source_files':len(owned),'reviewed':reviewed,'partitions':len(state['partitions']),
                  'candidates':len(state['candidates']),
                  'confirmed':sum(c['status']=='confirmed' for c in state['candidates'].values())}))
