# Integrate Hybrid Phase 2 Transaction Lanes

## Authoritative checkpoint (2026-10-06)

Status: Current fused JSON replay completed; general three-lane scheduler deferred.

JSON facts and ordinary rules share the existing canonical Phase 2 schedule. JSON-only publication need not own complete raw bytes; forced raw visibility and DetectionOnly syntax fallback obtain canonical bytes when required. True simultaneous raw/structured/compatibility scheduling and cache-free forwarding remain separate future work, not acceptance gates for the current Full JSON implementation.

Current implementation, verification and independent review are tracked in
[plan 16](16-json-capture-wiring-and-closure.md) and
[validation task 06](06-json-validation-and-benchmark.md).
These checkpoints supersede the historical interfaces and acceptance statuses
below. Historical measurements retain their original scope and revisions.

## Historical checkpoints (superseded design)

## Meta

| Key | Value |
|-----|-------|
| Status | in-progress (exclusive JSON API exists; broader hybrid gate open) |
| Priority | P1 |
| Depends on | `03-json-value-path-publisher.md` |

## Goal

### Current boundary (2026-10-05)

The engine already implements start/feed/finish JSON collection and canonical
ordered replay, including eligible ordinary units. It is an exclusive bounded
JSON transaction lane: initial unsupported admission can choose complete-body
publication, but lanes are not fed simultaneously. The first Gateway adapter
uses a complete canonical BodyStorage snapshot before dispatch. The work below
describes remaining broader hybrid acceptance, not a prerequisite permission to
duplicate request caches or move admission into post-forward relay callbacks.

Allow one transaction to advance raw streaming, structured streaming, and an
optional bounded compatibility snapshot from the same request chunks without
duplicating input or changing source-order semantics.

## Work

- Extend the compiled Phase 2 summary with the exact lanes required by one
  generation.
- Construct only the lane state required by the selected generation.
- Feed each incoming chunk once to every active lane.
- Keep the complete raw AC stream active after the compatibility snapshot
  reaches its independent limit.
- Keep the structured parser active or fail according to its own limit policy;
  never inherit the snapshot oversize outcome accidentally.
- Merge lane results through one source-order Phase 2 evaluator.
- Permit pre-forward intervention only for compiler-proven monotone rule units.
- Finalize all remaining rules exactly once at structured completion or EOS.
- Release parser/token scratch no longer needed after Phase 2.

## Tests

- Raw-only, JSON-only, snapshot-only, and every mixed-lane combination.
- Independent exact-boundary and first-over tests for every byte/work budget.
- Same-chunk and cross-chunk competing matches with source-order actions.
- Detection-only, disruptive, negated, chained, and stateful rule units.
- Abandoned input and repeated terminal-event rejection.

## Exit gate

The transaction executes all required lanes from one byte stream, retains only
declared bounded state, and matches the complete-body reference for the
supported profile.
