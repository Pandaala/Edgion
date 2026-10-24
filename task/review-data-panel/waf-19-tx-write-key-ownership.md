# TX write-key ownership

Status: deferred for user decision. No runtime design selected.
Source baseline: `a720811b3`.

## Evidence and scope

In `Transaction::apply_member_actions`, deletion already uses `template_operand`
to borrow validated static keys. Assign/Add/Subtract still call owned template
expansion before value processing. This was deliberately outside the earlier
deletion optimization's scope.

`TxState::set` resolves the incoming key and uses canonical case-insensitive
lookup. On a hit, it replaces only the stored value; the newly expanded key is
discarded and the original key spelling/owner remains. On a miss, it retains the
incoming key with the value. A nonempty static write key is therefore copied to
temporary owned storage even when the entry already exists.

This is an allocation/ownership candidate, not a correctness finding. Dynamic
keys still need canonical expansion. Arithmetic actions also perform a preceding
get; eliminating or merging repeated lookups is a separate mechanism and is not
part of this task.

## Decision alternatives

- Retain current ownership if a more general setter contract costs more complexity
  than the measured update-path savings justify.
- Evaluate borrowing static generation bytes until the canonical setter determines
  whether insertion needs an owned key, reusing existing template validation and
  expansion rather than a second interpreter or setter implementation.

No alternative is approved. Do not introduce a persistent generation-borrowing
MatchValue variant, a key cache, another TX map or a compatibility layer. The
canonical setter also receives capture keys represented by owned bytes or sealed
structured-result handles; any API design must account for those callers.

## Ordering, failure and ownership gates

Keep key semantic validation before value processing, per-member expansion limits,
setvar-action admission, ordered visibility of earlier actions, macro reads and
arithmetic conversion/overflow behavior. Existing keys retain their original
spelling and owner on mixed-case replacement. New entries remain owned by the
staged transaction; no temporary input reference may escape.

Allocation failures are an explicit decision boundary: avoiding an update-key
allocation removes that failure opportunity. Deferring an insertion-key copy
would move a fallible allocation relative to value evaluation, lookup, variable
limit checks and vector growth. Review those changes explicitly; this proposal
does not promise identical allocation-failure precedence. Preserve semantic error
and metered-read ordering apart from any separately accepted allocation change.

Keep canonical stored-key validation and structured-result work charging,
including sticky failures. Do not reuse an earlier arithmetic lookup to skip a
later setter lookup or sealed-handle read. Preserve first-key identity, captures,
staged commit/failed-chain rollback and later-phase observations.

Semantic TX payload/byte charges must stay exact and distinct from physical heap
capacity and allocation overlap. Review both successful insert/update and failure
cleanup; fewer temporary bytes do not authorize relaxing existing limits or
dropping retained owners. No new observability, request logging or unsafe code.

Before implementation, freeze an exact allow-list and obtain independent IMPL
plan/final review. Tests must cover static/dynamic insert/update, mixed case,
empty and missing keys, chained failures, captured shared keys, ordered macros,
action/variable/byte/expansion limits and arithmetic failure. Add allocation-owner
evidence for the selected design without inferring throughput or RSS from counts.
Benchmark representative update-heavy and insert-heavy workloads, including short
keys and dynamic templates, before asserting a net performance benefit.

## Sources and current verification

- `edgion-waf/src/seclang/transaction.rs`: apply_member_actions, template_operand,
  expand_template, TxState::set/find_entry and MatchValue.
- [Fused structured-result ownership](../../skills/04-review/waf/regression-guards/fused-json-inspection.md).
- [Staged control and action scope](../../skills/04-review/waf/regression-guards/request-scoped-control-actions.md).

Fresh unchanged-source checks passed six setvar-selected tests and 52 Gateway WAF
tests, formatting and workspace all-target check/Clippy with existing warnings.
These establish baseline behavior, not acceptance of a setter API or allocation
policy. No prototype, benchmark, full suite or live integration ran.
