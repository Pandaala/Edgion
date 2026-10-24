# JSON structural key comparison work batching

Status: deferred for user decision. No runtime design selected.
Source baseline: `c3278ca6a`.

## Evidence

Active full WAF JSON inspection constructs the canonical JsonChecker through
`json_inspection_runtime.rs`. Its reducer inserts decoded object keys into a
bounded AVL index. `Reducer::compare` in `json_checker.rs` validates the stored
key range, then walks paired bytes and calls `Budget::work(1)` before each byte
comparison. It returns on the first mismatch. Equal paired prefixes incur one
additional work charge before comparing lengths, including empty keys.

Long common prefixes repeat checked arithmetic, counter writes and limit checks
for every byte at each visited tree node. This is a source-level CPU candidate,
not an unbounded-memory or correctness finding. Current key ownership is required:
new keys enter the scoped arena, duplicates reuse identity, and object completion
drains sorted receipts before restoring arena marks. That design remains intact.

## Decision alternatives

- Retain the scalar implementation if typical keys are short or measured savings
  do not justify a more complex comparison/accounting seam.
- Investigate bounded comparison batches that recover the exact first differing
  byte and charge the same logical work. The batch size and allowance calculation
  need explicit review; a whole-slice ordering result alone does not reveal how
  many bytes the scalar implementation would have charged.

No alternative is chosen. Do not replace the AVL index, introduce hashes or a
comparison cache, retain another key copy, or alter decoded-key equality. This
task is separate from JSON-01 profile worklists and JSON-02 DSL validation-only
parsing. The earlier structural ownership audit does not authorize deleting
ordered work charges merely because node records are Copy.

## Failure and accounting gates

`Budget::work` first computes and assigns the checked sum, then checks the limit.
A work-limit failure therefore retains the failed increment; arithmetic overflow
returns without assigning. Starting within the limit, a scalar comparison fails
at the first excess unit, not after charging an entire remaining suffix. These
semantics differ from the lexical stream's success-only `charge` helper.

Preserve range-validation and range-arithmetic errors before comparison work,
first-mismatch early return, the final length-comparison charge, and exact partial
usage on failure. A batch must not scan an arbitrary suffix before discovering
that no work allowance remained. Preserve overflow versus limit precedence even
at maximum counter values; saturating arithmetic must not merge those outcomes.
Keep downstream insertion/mutation order, duplicate replacement, sorted output,
sticky failure and receipt abandonment unchanged. Physical capacity checks and
the current body-storage policy remain independent.

Any implementation needs a frozen allow-list and independent IMPL plan/final
review. Differential tests should compare scalar and candidate ordering, result,
work usage and error state across equal keys, prefix-only length differences,
early/late mismatches, decoded duplicate keys, empty keys, long shared prefixes,
batch boundaries, exact/one-short allowances and counter overflow. Whole-checker
tests must retain all rotations, nested duplicate replacement and cleanup gates.
Use test-only probes rather than request-path logging or new metrics.

Measure short and long keys, hit/miss ratios, sorted/reverse/random insertions and
duplicate-heavy objects, including failure paths. Report comparison cost and full
checker cost separately. No throughput, allocation or RSS benefit is established.

## Sources and current verification

- `edgion-waf/src/seclang/json_checker.rs`: compare, work, insert and cleanup.
- `edgion-waf/src/seclang/json_inspection_runtime.rs`: active checker ownership.
- [Fused JSON inspection contract](../../skills/04-review/waf/regression-guards/fused-json-inspection.md).

Unchanged-source verification passed 28 checker, 37 stream and 52 Gateway WAF
tests (117 total), formatting and workspace all-target check/Clippy. Existing
Clippy warnings remain. These checks validate current behavior, not a batching
design. No prototype, benchmark, full suite or live integration ran.
