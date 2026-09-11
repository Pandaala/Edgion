# Integrate Phase 4 with RawWindow

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `08-response-rule-classifier.md` |

## Goal

Execute supported Phase 4 WAF rules through Edgion's canonical async response
processor and `RawWindow` without response aggregation or per-chunk allocation.

## Work

- Add response stream state to the pinned request WAF context.
- Add a dedicated WAF response processor instead of calling the current empty
  EOS response callback per chunk.
- Reuse the single-raw `RawWindow` path and prepare all scratch before chunks
  arrive.
- Feed exact raw phrases through constant-state AC scanning.
- Retain only compiler-declared finite overlap/window bytes.
- Resolve every withheld generation explicitly as release, replace, or discard.
- Flush retained bytes exactly once at EOS.
- Handle cancellation and terminal ownership without double release or double
  Phase 4 execution.
- Disable the incompatible response cache path for Phase 4-enabled requests.
- Record one Phase 4 execution and bounded response-byte observations.

## Tests

- Match boundaries at every response chunk split.
- Release/replace/discard, cancellation, upstream error, downstream close, and
  exact-once EOS flush.
- Header-committed intervention behavior and detection-only mode.
- No allocation after processor setup on the ordinary per-chunk path.
- Retained memory bounded independently of total response length, including
  long downloads and SSE.

## Exit gate

Supported Phase 4 rules run through `RawWindow` with bounded memory, no full
response cache, no ordinary per-chunk allocation, and exact terminal ownership.
