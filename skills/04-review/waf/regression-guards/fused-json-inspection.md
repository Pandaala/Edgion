---
name: fused-json-inspection
description: Review native WAF JSON dispatch, bounded retention, ordered replay and Gateway source ownership.
knowledge_type: regression-guards
---

# JSON inspection uses one grammar pass and bounded selected retention

## Conclusion and scope

The native WAF JSON path uses JsonStream and JsonChecker to validate a complete
source without a DOM or complete field-value mapping. Static predicates run as
semantic values complete. Transaction effects run later in canonical rule order.
Production edgion-waf does not depend on serde_json; test oracles may use it.
This is a JSON execution contract, not a claim that every upstream CRS rule is
supported or that arbitrary predicates visit every character only once.

## Rationale and evidence

The contract follows the authorized no-full-materialization design and the
canonical [HTTP body ownership rules](../../../01-architecture/00-common/05-unification-points.md).
Implementation ownership is split deliberately:

- json_stream.rs validates grammar and borrows contiguous unescaped tokens.
  Token decoding uses 1024 inline bytes; checker keys and paths each use 256.
  json_scratch.rs centralizes checked capacity arithmetic. Fixed inline storage
  is separate from retained heap budgets; first spill admits its full allocation.
- json_checker.rs maintains a reusable path and bounded object-key AVL index.
  Nodes and key bytes use shared contiguous arenas with per-object marks; closing
  a child truncates its live ranges while preserving parent members and reusable
  capacity. Complete-source adapters may use bounded initial capacity hints.
  Node hints are disabled unless a conservative retained-capacity bound fits;
  actual copy work remains subject to the strict cap.
  Duplicate identity is decoded key within one object instance. Replacement
  discards the preceding subtree's summary and provisional results; dotted
  external-name collisions remain separate candidates.
- json_inspection_plan.rs precompiles selectors, shared transformation profiles
  and exact named demands under compiler budgets.
- json_inspection_collector.rs and json_inspection.rs retain sparse predicate
  outcomes, captures, required name metadata and explicit deferred values.
  Same-candidate/profile/stage matches share one owning outcome's payload; aliases
  have independent captures. Generational stage tokens reject stale/cross-scope
  reuse. Whole-profile/candidate cleanup is essential: introducing independent
  outcome deletion requires a new ownership design, not reusing current aliases.
  Broad-only dispatch is preordered at compilation; mixed selectors retain their
  per-candidate selection and deduplication.
- json_inspection_runtime.rs publishes sealed selected facts and optional raw Bytes, reconstructs selected
  outcomes and preserves ordered chain, capture, TX and control semantics.
  Transaction selection uses a candidate cursor and bounded reusable reference
  scratch, charged to aggregate retained bytes even while taken for evaluation.
  New-unit clones omit old match observers; chain-branch clones remain complete.
  Static JSON match values use opaque handles into the sealed request-owned result
  arena. Replay, match observers and chain clones share the payload; owned dynamic
  values retain fallible copies. All match reads and macros resolve the handle with
  owner/range and sticky physical-work checks. Minting before sealing or reclaiming
  result bytes afterward would invalidate this ownership contract. Semantic value
  lengths remain charged even when storage is shared; actual metadata sizes are
  charged consistently on ordinary and fused paths.
  Match vectors grow only for actual hits. Cursor traversal may change competing
  physical-limit versus selector-error timing compared with eager enumeration.

Physical source/work/retention failures are immediate and sticky in the engine.
The Gateway latches them and continues only bounded canonical capture so transport
and original complete-source prelude failures retain their priority at EOF. Speculative
operator errors surface only when canonical selection and scheduling reach them.
Later transformation validation may override an earlier provisional predicate
hit. Duplicate replacement never erases syntax failure or spent physical work.
Final logical publication limits have explicit priority rather than an all-node
event tape preserving the deleted DOM's competing-error order.

Resource Full admission rejects unsupported JSON dependencies for both inline
and resolved bundles, including omitted JSON overrides. There is no DOM fallback.
Gateway preflights after phase-one controls and feeds borrowed actual chunks under
the independent physical cap. A scoped JsonInspectionSession owns persistent
collector state and a mutable transaction borrow; each feed constructs only a
short-lived borrowed sink/context. True EOF, exact captured/available parity and
the complete BodyAccounting prelude precede sealing and Phase 2. Failed completion
is terminal; unfinished Drop poisons publication. Raw visibility obtains the same
canonical immutable Bytes owner only when required by the existing policy.

## Alternatives and limits

Only demanded values are retained, but memory is not constant or limited to two
buffers: key identity, candidate order, profile state, matches and captures have
independent bounded storage. Dynamic broad value operands requiring every value
are rejected. Dynamic names, narrow length recipes and finite named dependencies
have separate execution paths. Raw whole-source operators may scan the original
bytes later without another JSON grammar pass.

Gateway inspection now consumes real capture chunks before transport EOF. Cached
bodies use bounded immutable snapshot chunks with pre-IO size admission and actual
reader EOF/parity. JSON-only spill requires no complete materialization; forced raw
visibility and DetectionOnly syntax fallback may require one. BodyStorage remains
the sole forwarding/retry cache; no cache-free forwarding is claimed. Future header
or nested-JSON adapters require explicit source ownership and aggregate limits.

The existing inspected-byte counter uses exact lexer-admitted source deltas;
continued capture after failure is excluded. Provisional RAII charging includes
known current parser/collector capacities plus canonical logical TX accounting and
hands off once to installed transaction charging. Errors and cancellation release
the charge. Inline storage, BodyStorage, allocator overhead and RSS are excluded;
independent component peaks are not a simultaneous aggregate peak.

## Verification and re-evaluation

JSON classification reports must distinguish historical analyst conclusions from
the replacement implementation. CAND-local-001 corrected retired narrow-lane,
always-copy, production-serde-number and fixed-depth descriptions in
`tasks/pending/waf-json-rule-classification/`. Its current contract and inventories
are bound to a separately hashed source manifest; each member assessment leaves
original-unit compiler admission and runtime acceptance explicitly untested.
Original reports and corpus bodies remain unchanged. Verification defaults to
read-only current scoped hashes or historical `git show` evidence and publishes
only explicitly requested separate result files. Isolated regeneration must
preserve the current overlay and original attribution. Re-evaluate the assessment
and source manifest after any relevant compiler, parser or Gateway entry change;
never infer current support from a historical category or old pass record.

The classification reconciliation remains a frozen `5554f0c` baseline after
native XML extends shared compiler/collector/transaction and Gateway files.
Current-worktree hashes can therefore differ intentionally; do not refresh the
manifest to imply new original-rule validation. XML shares the selected-result
and replay engine under an explicit domain; see
[the XML regression guard](native-xml-inspection.md) for its separate projection
and physical-error contract. JSON-only summary optimizations remain JSON-only.

The historical `tasks/pending/waf-json-checker/` records are retained in the
working tree. Capture wiring and closure were tracked in the historical
`tasks/waf-todo/16-json-capture-wiring-and-closure.md` task, which is no longer
present in the working tree.
Prior test and measurement results below are historical evidence, not proof of
the new entry. Remaining WAF work is tracked in the
[current execution queue](../../../../tasks/waf-todo/README.md).

C1 independent review approved the replacement and legacy assertion mapping;
508 WAF library tests passed with two existing ignored cases, and API 4/4 passed.
C2 independent reruns passed resource WAF 25/25, CRD 1/1 and Gateway WAF 24/24;
strict affected-crate Clippy passed. Final verification also passed workspace
check/Clippy, 170 request-body tests and fresh WafDynamic live integration.
The configured native CRS profile passed 4,741 admitted stages, including 896
negative stages, after matching the upstream FTW XML-attribute test opt-in.
This is not full-CRS compatibility or production throughput evidence. Scoped
release measurements and their substantial remaining allocation costs were
recorded in the historical initial results (`13-results.md`).
That historical measurement is not fresh evidence.

Re-evaluate when selector admissibility, duplicate ordering, transformation
validation, result lifetimes, phase controls, limit precedence or BodyStorage
materialization changes. Preserve the mirrored checker/collector/runtime tests
and Gateway completeness, one-read, source-lifetime and error-classification
regressions.

The unified follow-up design and review gates were recorded in the historical
optimization specification (`15-optimization-spec.md`).
The allocation follow-up was recorded in the historical capture/query plan
(`21-capture-query-and-transaction-plan.md`).
Re-evaluate physical capacity/work thresholds when inline sizes, retained scratch,
shared outcome destruction or broad dispatch ordering changes. Preserve successful
semantic results, strict charged limits and terminal failure behavior; do not infer
identical allocator-dependent admission from unchanged logical JSON rules.

The common transformation walker also borrows unchanged ASCII-lowercase input.
The no-uppercase preflight proves identity; finding uppercase proves change and
replaces the output equality comparison. Input/output work and checkpoints remain
charged, and later transforms still validate. This does not borrow persisted match
payloads from the body or extend source lifetime. Preserve both multiMatch modes,
exact work/output bounds, repeated-transform storage and final borrowed/owned behavior.

Local key-tree links use private NodeId(NonZeroUsize), with checked index-plus-one
encoding so optional links occupy one machine word. Bounds-checked node access
and scoped arena lifetime remain mandatory; these IDs are not persistent sink
receipts. Key offsets/public limits remain usize. Growth admission uses actual
Member/Frame size, including when layout changes reduce physical work or heap use.

The sealed-value follow-up and scoped verification were recorded in the historical
shared replay results (`18-shared-replay-results.md`).
Re-evaluate when result arenas become mutable after publication, request publication
can be replaced while match observers survive, or matches can escape the transaction.
Keep shared-pointer, cross-owner/range, macro/chain/rollback and logical TX budget
tests. Fewer allocations do not imply lower cumulative bytes for very short values.


The allocation/scan follow-up was recorded in the historical approved plan
(`19-allocation-and-scan-plan.md`) and results (`20-allocation-and-scan-results.md`).
Member-match capacity now survives unit/chain evaluation and remains charged while
loaned. Sparse qualified names are pooled only for surviving hits; match names,
MATCHED_VARS keys, capture values and TX capture clones use sealed handles.
All runtime reads remain metered; tx_value alone is immutable observation with
owner/range validation. Semantic length admission remains independent of sharing.
Large BytePool classes use checked page rounding and charged sorted metadata;
small classes retain fixed heads. Batch ASCII scanning preserves sequential
special-byte and near-limit errors, verified against a test-only lexical oracle.

Accelerated no-hit JSON-tail replay requires a sealed completed sole-value-profile
proof, never-allocated outcome/profile/change pools, static unmodified Args selection,
identity checks and successful logical/physical/hypothetical-selection preflight.
It charges the current cursor's N selection and 3N replay work without
materializing body candidates. Shared XML/JSON read reuse removed the None
selector's redundant name read and static replay's duplicate candidate/profile
reads; the historical pre-optimization ledger was 2N/5N. This remains a JSON-only
proof. Its counts must follow actual canonical reads, with exact/one-less and
forced-cursor differential coverage, never a blanket admission waiver.
Queries are selected and evaluated normally: full Q+N selection,
geometric capacity and possible-query-match admission precede matching; virtual
body capacity exists only in MatchStorage.fixed_bytes. Persistent scratch retains
actual capacity. Query errors or early stops do not charge JSON-tail replay. Later
actions or rules may gain actual retained headroom; logical TX/capture/count/work
limits stay unchanged. Ambiguous shapes, changed/error profiles, discarded hits
and uncertain limits fall back before counter mutation. Reference always
uses the cursor. Re-evaluate when runtime counters, profile completion, selection
growth or scratch lifetime changes; preserve forced-cursor differential tests and
source-local test-oracle layout exceptions.


The capture/query follow-up was recorded in the historical approved plan
(`21-capture-query-and-transaction-plan.md`) and results
(`22-capture-query-and-transaction-results.md`).
Capture extraction uses a shared admission/enumeration helper with an owned
ordinary policy and a JSON relative-range/fingerprint policy. Stored ranges
resolve against the owning stored stage match, never a fabricated subrange Lease.
Numeric and named aliases remain independently charged by semantic length. Only
Owned payloads are freed; profile/candidate destruction still owns range lifetime.

New-unit execution loans persistent TX entries while hiding previous match
observers and preserves the original total in an accounting placeholder. Every
Result exit restores TX before propagation. Stage mutation clones start only
after first-member hits; chains reuse those results without a second evaluation.
Sparse qualified names use an optional pair index on the candidate and immutable
Args/ArgsNames spans. Full candidate owner/slot/generation validation precedes
index and byte-bound checks. Use actual size_of for physical admission.

Marker-free RemoveNulls, HtmlEntityDecode and UrlDecodeUni borrow unchanged input
only within the output cap. Marker presence or oversized input follows the old
materializing path, preserving transformation errors and checkpoints. Re-evaluate
TX loan self-state reads, capture owner cleanup, virtual capacity, semantic alias
limits and unchanged-transform output precedence whenever these paths change.

Positive capture execution deliberately retains boolean miss checking followed by
capture extraction on hits. A measured single-search prototype was rejected because
all-miss timing regressed across both run orders; semantic correctness alone does
not justify replacing this path. Capture-program interning was implemented in the
subsequent bounded compiler round below.

The subsequent borrowing/scratch work was recorded in the historical plan
(`23-borrowing-and-scratch-plan.md`) and results (`24-borrowing-and-scratch-results.md`).
Single-match final transformation output borrows unchanged input or moves changed
scratch; multiMatch keeps its intermediate stages. Temporary JSON capture keys
use numeric/program-group descriptors in a reusable builder. Resolve named keys
only against its bound generation and copy at result transfer. Charge actual
high-water builder capacity dynamically during transformation/predicate admission;
clear logical contents on every extraction/transfer error and drop before seal.
Persistent TX capture keys remain separate from these temporary descriptors and
now use the sealed-handle policy described below for static JSON results.

TX cached payload totals are exact u128 with checked mutation deltas. Semantic
lengths, actual metadata and observer-loan restoration still determine admission.
Final usize saturation must not corrupt future subtraction. Preserve frozen
representable-state recount and independent wide overflow oracles. Cached counters
increase TxState/ChainBranch layout; use actual sizes and report peak tradeoffs.

Buffered ASCII batching scans only budget-admitted current spare capacity after
classifying the existing current byte. Empty-token borrowing and special-byte
fallback retain their order; preserve sequential counters, spill trajectory and
competing errors. Duplicate AVL unwind skips unchanged ancestor work, with no
refund of spent search/discard work or change to logical replacement semantics.

Capture programs are now interned by exact source under bounded canonical work
and temporary capacities. Preserve occurrence gates, every reuse requirement and
removed-rule validation; publication follows successful compilation. Carry aggregate
work through grouping and JSON planning. Full group identity and independent member
replay remain mandatory. JSON scan-cache demand counts unique Matcher IDs only.
Re-evaluate compiler peaks, generation identity, high-water headroom and error
cleanup when changing any of these mechanisms. Directed identical-regex benchmarks
do not establish broad CRS speedups.

The capture-key and candidate follow-up was recorded in the historical plan
(`25-capture-key-and-candidate-plan.md`) and results
(`26-capture-key-and-candidate-results.md`), removed with the old task tree.
The recorded final independent Astra code/artifact review approved the disclosed
allocation, CPU and descriptor tradeoffs. Recorded verification passed WAF tests
(639 passed, two ignored), API4, workspace checks/guards and all 1,240
paired/three-version rows. These are historical results, not a fresh rerun.
Static JSON capture keys share opaque sealed handles through replay, TX entries,
capture cleanup lists and both clone policies. Ordinary captures remain fallibly
owned and move their existing builder vector. TX key lookup, mutation, selection
and macros validate shared keys with canonical metered reads; public tx_value alone
uses immutable observation. Overwrite preserves original key spelling. Logical
payload lengths remain charged, while actual entry and cleanup-element sizes grow.
Keep case-alias cleanup, numeric spelling, later-phase, ownership, sticky-work and
rollback tests. Shared bytes do not imply free descriptor storage or key reads.

Only the final member consumes an already staged chain frontier in its original
winner order. The original frontier accounting checkpoint remains before selection.
Nonfinal breadth-first cloning and first-member observer loans retain their contract;
live TX is replaced only after successful commit. Preserve the frozen clone-frontier
differential tests and exact/one-less admission, action and rollback boundaries.

Candidate's three private head/next links omit their fixed pool owner and retain
the complete u32 slot and generation. Reattaching owner never refreshes generation.
External IDs, views, receipts and other records retain full identities. Validate
external owner/slot/generation/occupied state before following local links, and
retain profile/outcome parent checks. Physical growth uses actual production types;
the measured 64-bit Candidate and Slot<Candidate> are 160 and 176 bytes. Re-evaluate
packing, lifecycle and physical failure thresholds when any link or pool changes.

## Selective tokenizer reuse by AI usage observation

`JsonStreamSelective` is an opt-in wrapper over the same transport-neutral JSON
grammar. It captures bounded decoded keys and raw number spellings; unselected
strings are UTF-8/escape/surrogate-validated and reported as discarded metadata.
Skipped content is subject to source/decoded/work limits without inheriting the
key-capture cap. No frame-sized continuation copy or provider-specific logic is
introduced into the WAF crate. The AI caller owns its profile and selected paths.

Default `JsonStream` token borrowing, capture, counters, errors, limits and WAF
verdicts remain unchanged. Selective coverage is additive: preserve the existing
test identities and compare whole, byte and mixed schedules under every budget.
The fresh supporting full WAF library run for Task 02 passed 642 tests with two
ignored; this is a dated implementation result, not a substitute for revalidation.
