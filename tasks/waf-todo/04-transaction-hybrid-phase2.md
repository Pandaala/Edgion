# Integrate Hybrid Phase 2 Transaction Lanes

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `03-json-value-path-publisher.md` |

## Goal

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
