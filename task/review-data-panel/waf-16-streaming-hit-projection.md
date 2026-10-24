# Streaming phrase hit projection

Status: conditional dormant-API investigation for user review. No runtime change.
Source baseline: `7b873d7a4`.

## Current Gateway reachability

Revalidated at `6814acb0d`: production WafPlugin does not call
start_request_body_stream or enable_request_body_streaming. Its active request
path executes the pre-dispatch prefix evaluator or full-body evaluator. The
context flag defaults to false, and run_waf_body_stream returns immediately unless
enabled. Adapter/context bridges and test activation remain in the tree; their
presence does not establish active Gateway traffic through this lane.

The [prefix-before-dispatch contract](../../skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md)
records why the old post-dispatch raw handler was replaced: a late stream verdict
cannot retract upstream headers or bytes already forwarded. This task authorizes
neither reactivation nor a change to that admission boundary. Revisit the candidate
only in separately authorized work with a demonstrated consumer. It is not a
current Gateway hot-path performance backlog item. The earlier stream-only outer
iterator improvement supplies no evidence of an online Gateway gain; the shared
native PatternSet iterator also serves active non-streaming scans independently.
Continue current Gateway optimization investigation through prefix/full execution.

## Evidence

The dormant leaf raw request-body lane reaches the native phrase scanner through
Transaction::execute_request_body_stream_chunk, CompiledMatcher::scan_stream_chunk
and PhraseGroup::scan_stream. The native try_stream_pattern_set_chunk accumulates
matching IDs in PatternSet while retaining automaton state across chunks.

After each successful native scan, PhraseGroup maps every cumulative local hit
into MatcherStreamState.matched_words. After a successful matcher group scan, the
transaction maps every cumulative outer hit into member_positive. Previous chunks'
hits are therefore projected again, even when a later chunk adds no new hit.
Both iterators now use the canonical bounded set-bit traversal; that skips absent
bits but does not remove repeated projection of historical hits. All current
state is bounded; this is a CPU optimization candidate, not an unbounded-memory
or correctness finding.

## Decision alternatives

- Retain cumulative projection if measured savings do not justify added state,
  branches or changes to the existing error boundaries.
- Evaluate owner-local per-chunk scratch. Clearing local bits produces this
  chunk's hits, not first-ever hits: a recurring pattern can still be projected
  again. Compare bitmap clearing cost and the still-cumulative outer projection.
- Evaluate a bounded first-hit delta or newly-set-ID API, explicitly choosing
  where local-to-global and global-to-member publication occurs. Account for
  any additional pending/seen storage and capacity; do not introduce an unbounded
  occurrence list or a second independent matcher.

No alternative is selected. Preserve the native public cumulative PatternSet API;
an internal projection optimization must not silently change its contract.
Measure local and outer projection separately across chunk sizes, pattern counts,
hit density, repeated occurrences and long tails after early hits. Include cold
setup, extra clearing, retained capacity and end-to-end SecLang cost. No net CPU,
allocation, RSS or throughput benefit has been measured for this candidate.

## Semantic and admission gates

Keep cross-chunk automaton continuity, overlapping/suffix matches, duplicate
pattern IDs, case policies, source-order fanout and cumulative member decisions.
Preserve the compiler-selected raw lane and its active-control rejection, the
sole-positive-member early completion optimization, EOS decisions for negation,
empty chunks, finished-state errors and generation/shape checks. This creates no
general response-body buffering or new request-body storage.

Error state is layered, not whole-chunk atomic. Native scanning may update state
before failure. Local-to-outer mapping starts only after native success; a
sensitive phrase group can update the outer bitmap before an insensitive group
fails. Member mapping starts after a complete matcher group succeeds, but earlier
matcher groups can already have updated members when a later group fails.
Preserve these boundaries and error ordering; a direct callback must not publish
member results earlier merely because it sees a match sooner.

Retain body caps, checked offsets, per-group byte-work charges, retained-byte
admission and actual capacities. Faster projection does not justify lowering
logical charges. Any additional storage must have an explicit bounded owner and
reconciled accounting. Keep observability unchanged and avoid request logging,
profiling metrics, shared mutable scratch or unsafe code.

Before implementation, freeze a cross-crate allow-list and obtain independent
IMPL plan/final review. Compare against current behavior for repeated and new
hits, tiny/empty chunks, boundary-spanning matches, multiple case groups and
members, negation/EOS, early completion, invalid owners/finished state and exact
or exceeded work/body/retained limits. Add directed error-state checks for the
chosen projection mechanism; existing happy-path tests alone are insufficient.

## Sources and verification

- `edgion-waf/src/aho_corasick/stream.rs`: cumulative scan and state validation.
- `edgion-waf/src/matcher.rs`: PhraseGroup and CompiledMatcher streaming owners.
- `edgion-waf/src/seclang/transaction.rs`: raw-body setup, per-group charges and publication.
- [Streaming control boundary](../../skills/04-review/waf/regression-guards/request-scoped-control-actions.md)

The initial audit's unchanged-source checks passed 72 native matcher, 47 combined matcher and
three raw-body streaming transaction tests (122 total). Two matcher release
witnesses remained ignored. Formatting and workspace all-target check/Clippy
passed with existing unrelated warnings. These checks describe the current
implementation, not an accepted delta design. No prototype, benchmark, full suite
or live Gateway integration ran.

Reachability-correction verification passed 52 Gateway WAF tests, three canonical
prefix-adapter tests and 24 leaf prefix tests (79 total); one leaf release witness
remained ignored. Formatting, workspace all-target check/Clippy and agent-doc
validation passed. Dormancy follows from the inspected production activation
calls and flag gate, not from these test counts. No source or test body changed.
