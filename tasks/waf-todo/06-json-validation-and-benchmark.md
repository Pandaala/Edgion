# JSON Validation and Performance Closure

Status: completed for the current fused Full JSON capture entry.
Depends on: task 05. Scope: current fused Full JSON capture entry.

## Accepted design and scope

Reuse the native parser, compiled rule plan, selected result store and canonical
transaction replay developed in earlier rounds. Inspect actual transport chunks
through the one canonical request BodyStorage capture loop. No serde_json/DOM,
second cache, whole field map or new parser is introduced. EOF-dependent effects
remain before upstream dispatch. The current fused Full JSON path is already
selected by effective processor; this is not an opt-in classifier promotion.

Two counters remain independent: complete captured source length and actual
lexer-admitted bytes. After inspection failure, bounded transport capture continues
without more grammar work. Complete prelude and transport errors preserve their
priority. DetectionOnly syntax fallback follows the existing raw visibility policy.
Known provisional capacities plus logical transaction accounting are charged on
the existing runtime gauge with RAII handoff; the gauge is not RSS.

## Fresh final verification (2026-10-06)

- Complete WAF library: 641 passed, two existing ignored.
- Public JSON API: nine passed, including chunk partition parity, empty/abandon,
  prelude priority, terminal failed completion and raw-policy cases.
- Gateway WAF: 30 passed; real spilled capture, positive pre-EOF feed/charge,
  duplicate replacement, cancellation, competing failures and raw read policy.
- Adapter EOF/parity/pre-read-cap/late-reader-error regression: one passed.
- Strict WAF all-target and Gateway library Clippy, scoped formatting and diff checks passed.
- Directed development smoke: 96 forward and 96 reverse semantic comparisons
  passed. These development runs are validation, not performance evidence.

## Fresh closure evidence

- Workspace format, all-target compile/lint and all six Python guards passed;
  the affected benchmark was rechecked after its measurement-only correction.
- Gateway request_body170 and WAF context6 passed in addition to the tests above.
- Canonical rebuilt WafDynamic passed in 78 seconds, including all H1/H2 pairings,
  spill, pre-upstream denial, cross-chunk duplicate replacement and hot-reload pinning.
- M4 release: 192 directed rows across both orders plus 432 baseline/final owned
  ABBA rows; all assertions passed. Each row uses 31 samples and three warmups.
- All directed non-timing usage/work fields match across both orders. Owned capacity
  columns are unmeasured in the directed matrix; no physical-memory claim follows.
- Astra approved implementation, added boundary tests and final evidence. Its
  independent artifact audit confirmed pairing, arithmetic, identities and capacities.
  No blocking findings remain.

### Local CPU comparison

32 generated identical @pm hit rules with lowercase, On mode. Each number averages
reported p50 across two run orders; it is not pooled statistical inference.

| Fixture | Owned p50 ms | 512B p50 ms | 16KiB p50 ms | 64KiB p50 ms |
|---|---:|---:|---:|---:|
| small_plain | 0.2536 | 0.2560 | 0.2549 | 0.2558 |
| wide_no_hits | 12.5947 | 12.5508 | 12.4876 | 12.4435 |
| escaped_hits | 3.4070 | 3.4346 | 3.2946 | 3.4248 |
| duplicates_last_miss | 1.1219 | 1.1536 | 1.1274 | 1.1137 |
| malformed_tail | 0.6295 | 0.7034 | 0.6300 | 0.6256 |
| large_scalar | 0.4460 | 0.5202 | 0.4730 | 0.4715 |

The 10,000-field no-hit fixture is 280,001 bytes; the large-scalar fixture is
262,159 bytes. At 16KiB chunks, large-scalar p50 costs +6.0%; 512B costs +16.6%.
Its independent lexical peak is 523,969 bytes, structure peak 1,248 and result
peak 542,675. These are separate component peaks, not a simultaneous sum or RSS.
Full source IO avoidance does not eliminate cross-chunk token buffering or prove
lower aggregate memory for scalar-dominated input.

The existing owned entry's local ABBA p50 changes range -25.1% to +12.1%;
selected 32-rule accelerated wide-hit input costs +6.7%. This small matrix does
not establish universal CPU gain. Full tails, component capacities, source hashes,
binary fingerprints and commands are retained in
[the evidence directory](evidence/json-capture-2026-10-06/README.md).

The default benchmark had incorrectly required equal retained capacities across
Accelerated/Reference and reported Accelerated capacity on Reference rows. The same
measurement-only fix was independently approved and applied to both snapshots;
input and timed operations were unchanged, with original hashes/patch retained.

## Historical evidence and deferred work

The prior FTW profile (4,741 admitted stages, 896 negative) and capture/candidate
round's 1,240 paired rows are recorded under
[the fused checker task](../pending/waf-json-checker/waf-json-checker.md).
They do not prove this new capture entry's timing or transport semantics. Earlier
S18/S19 and full-snapshot checkpoints are retained in tasks 00–05 with historical
labels. They are not current activation gates.

Cache-free forwarding, global StreamingStructured promotion, simultaneous general
raw/structured/compatibility scheduling, XML and response RawWindow execution
remain future deliverables. Universal CPU improvement is not a closure criterion:
record any chunk/token overhead and retained-capacity tradeoffs explicitly.

Current plan and final command/artifact evidence:
[16-json-capture-wiring-and-closure.md](16-json-capture-wiring-and-closure.md).
