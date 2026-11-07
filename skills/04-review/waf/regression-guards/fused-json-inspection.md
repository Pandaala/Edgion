---
name: fused-json-inspection
description: Review native WAF JSON dispatch, bounded retention, ordered replay and Gateway source ownership.
knowledge_type: regression-guards
---

# JSON inspection uses one grammar pass and bounded selected retention

## Conclusion and scope

The native WAF JSON path uses JsonStream and JsonChecker to validate a complete
source without a DOM or complete field-value mapping. Reachable static predicates
run as semantic values complete. Transaction effects run later in canonical rule order.
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
  Duplicate identity is decoded key within one object instance. Every
  occurrence of a duplicate key is kept: the reducer reuses the key's AVL node
  and `finish` joins each completed value's summary and receipt after the
  earlier ones, so occurrences publish in document order under their key while
  object members keep canonical key order. Each occurrence is its own
  `ARGS`/`ARGS_NAMES` candidate with its own predicate facts, captures and
  counts (`&ARGS`, `ARGS_COMBINED_SIZE`, entry limits), as in ModSecurity's
  argument multimap; earlier provisional results are never discarded, and the
  checker no longer calls the sink's `discard`. A named macro such as
  `%{ARGS.json.k}` resolves to the first occurrence. More occurrences reach the
  existing entry, selection and result-store limits exactly as more distinct
  keys would. Dotted external-name collisions remain separate candidates
  (finding `waf3-02`, decision commit D-2).
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
hit. A later duplicate never erases an earlier occurrence's facts, a syntax
failure or spent physical work.
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

## Demand-driven scratch and static activation

Transaction matcher, boolean-regex and capture-regex caches use fixed Option
slots and initialize once at their shared first-use seams, including selectors
and capture extraction. They retain request-owned capacity until Drop. Full
generation scratch envelopes remain conservatively admitted, with actual Option
layout/capacity; the retained-byte gauge does not report measured heap.
A prior shared scan requires an initialized matching scratch slot.

`inspection_activation.rs` compiles bounded Phase2 forward successors and proves
singleton literal TX integer skipAfter gates stable across all Phase2 effects.
Unknown writes, dynamic keys that may alias, capture writes/cleanup, whole-rule
removal or TX-target removal preserve conservative execution. Non-TX target
removal cannot change TX-only gate selection. No CRS IDs, tags or level names
select the optimization. A once-per-inspection charged may-reach mask accounts
for earlier jumps, removed gates, every chain/name/value consumer and later phases.

Only static dispatch is filtered. Grammar, duplicates, candidate/name metadata,
counts, deferred and raw demand remain unchanged. StoreRecords owns the mask
through sealing. Canonical lookup checks it before shared-profile completion
can yield NoMatch; inactive/invalid access is sticky. The JSON summary guard is
also explicit and cannot swallow an inactive-fact error. Abandon clears the mask
before resetting retained usage. A successful active sibling does not authorize
reading a pruned group.

Avoiding unreachable work can turn a former default result-work failure into
success without raising limits. The private test-only unpruned oracle compares
ordered findings, TX/captures and later phases; its enlarged reference work cap
is never a production override. Exact compiler/constructor budget boundaries,
mask lifecycle and typed-control counterexamples are regression gates. Re-evaluate
the proof when adding TX-writing actions, capture aliases, target controls,
scheduler edges or consumers.

## Alternatives and limits

### Contiguous cached input

`waf-cached-json-input-borrowing` removes artificial token carry copies for
eligible cached JSON. The user authorized implementation based on deterministic
copy avoidance without making a throughput benchmark an adoption prerequisite.
The Gateway JSON consumer opts into a 128 KiB maximum contiguous input; XML and
other consumers default to the ordinary chunk contract. This is an internal feed
bound, not an increased body limit or an executor-fairness guarantee.

`BodySnapshotReader::try_take_contiguous_memory` succeeds only before reading a
nonempty single resident segment. It validates the source range and snapshot
length, shares the existing Bytes owner and advances only its own cursor. Edited
Inline snapshots and Memory SourceRanges qualify; disk, multiple segments,
already-started readers and oversized sources fall back without cursor changes.
Isolation continues to use the isolated owner, never the original allocation.
No joining, payload copying, disk materialization or second cache is permitted.

`consume_request_body_snapshot` admits the declared length before trying the
capability, validates its returned length before consumption, and still reads
through actual EOF with exact byte parity. A terminal read error or extra bytes
cannot be hidden by a successful contiguous take. Live capture continues feeding
real chunks into the same stateful inspection session. Failed or abandoned
inspection cannot publish provisional results.

Logical grammar, duplicate-occurrence publication and ordered rule semantics are unchanged.
Physical work and retention charge actual operations, so removing carry copies
can turn an old physical-budget failure into success without increasing limits.
Escaped strings, selected captures and deferred values can still allocate; this
is not whole-pipeline zero-copy. An immediately ready reader does not guarantee a
yield between chunks, and no new scheduling mechanism is introduced here.

Regression coverage lives in Gateway storage, session-adapter and JSON-capture
unit tests and the WAF JsonStream tests. It checks original backing addresses,
owner lifetime and isolation, edited/disk/multi-segment fallback, exact input
ceilings, invalid ranges, EOF/errors, long plain/escaped strings, duplicate keys
and malformed/truncated documents. A scoped token-copy counter distinguishes
contiguous borrowing from 64 KiB carry without claiming Gateway throughput or
RSS savings. Re-evaluate when reader capabilities, source ownership, synchronous
feed bounds, physical accounting or publication rules change.

Fresh targeted verification for this change passed 56 Gateway storage tests,
43 session-adapter tests, 12 JSON/XML capture tests and 32 WAF JsonStream tests.
The 96 KiB plain-string witness recorded zero token-copy bytes and zero token
heap retention for contiguous input; 64 KiB partitioning copied at least the
96 KiB token and retained at least that much token heap capacity. Matching
semantic events were checked independently of physical work/capacity counters.
These are deterministic mechanism tests, not throughput or allocator benchmarks.
Workspace all-target check/clippy, formatting, agent-doc consistency, SSA force,
metrics inventory, Pingora source policy, Gateway API transform contract and
unit-test layout checks also passed. Full workspace tests were not run.

### Other retention and source boundaries

Only demanded values are retained, but memory is not constant or limited to two
buffers: key identity, candidate order, profile state, matches and captures have
independent bounded storage. Dynamic broad value operands requiring every value
are rejected. Dynamic names, narrow length recipes and finite named dependencies
have separate execution paths. Raw whole-source operators may scan the original
bytes later without another JSON grammar pass.

Gateway inspection now consumes real capture chunks before transport EOF. Cached
bodies use bounded immutable snapshot input with pre-IO size admission and actual
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

## Canonical compiler arena capacity

ARCH-231 corrected a distinct base-compiler accounting gap: occurrence-sized
reservations for target bytes, target records/plans and transformation values/plans
were retained by the generation but charged only for unique inserted lengths.
Deduplication reduced logical inventory without releasing those backing allocations.

`MatcherCompileWork::reserve_canonical` owns all five arenas' capacity admission,
including publication-time literal/path `ctl` selector appends. It admits requested
growth before fallible reservation, reconciles actual capacity, and checks old/new
allocation overlap both before reservation and after excess-capacity reconciliation.
Within-capacity insertions add no second retained charge. Existing occurrence-sized
preallocation and canonical IDs/order/equality remain unchanged; corrected headroom
can reject tight candidates earlier or reduce optional acceleration under unchanged
limits. This is not a whole-compiler heap audit or an RSS/throughput improvement.

Compiler regression tests independently sum capacity times element size at grouping
and publication, separating replaced transformation storage and unrelated publication
records. They cover repeated/distinct literal/path plans, chains, empty/nonempty
transformations, ctl spare-capacity and repeated-growth appends, exact/one-less
compiler/generation/overlap limits, and arithmetic rejection before allocation.
Existing strict-selection, collision, ordering, optional-fallback and body tests
remain required. Allocator excess reconciliation is checked in source; tests do not
force a particular allocator to return excess capacity. Re-evaluate on any new
canonical arena writer, reservation policy or publication-owner change.

## Verification and re-evaluation

Historical rule classification is not compiler admission or runtime acceptance.
Retired narrow-lane, always-copy, production-serde-number and fixed-depth
reports were superseded by the current bounded implementation. The old analyst
reports, source snapshots and execution queues were removed during task cleanup;
this page retains the reusable contract rather than a task transcript.
Use `config/waf/crs-4.25.1-supported-v1/` and the canonical test fixtures for
source inventory and executable acceptance. Re-evaluate support against current
compiler/parser/Gateway code after relevant changes.

XML shares selected results and replay under an explicit domain; see
[the XML regression guard](native-xml-inspection.md). JSON-only summary
optimizations remain JSON-only. Structural JSONPath and unsupported broad
dynamic dependencies do not gain a DOM fallback through historical categories.

Prior test and measurement results below are historical evidence, not fresh
verification or a whole-Gateway performance claim.

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
refund of spent search work or change to duplicate-occurrence semantics.

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
introduced into the WAF crate. The AI caller owns its profile and selected paths. Task 06 adds optional selected
string capture with a caller-declared decoded-byte cap. Its default remains discard;
selected strings use the same UTF-8/escape/grammar and retained budgets before capture.
Native Responses captures only short status/type metadata, never complete output.

Unselected plain strings batch admitted ordinary ASCII runs without retaining
their text. This path excludes keys, selected string capture, escapes and pending
UTF-8 continuations. Admission accounts for source, decoded and work limits and
both discarded-summary counters before advancing. Special bytes and exhausted
budgets return to scalar processing, preserving partial charges and error order.
This changes parser dispatch cost, not token capacity or the declined growth-hint
policy. Differential tests compare each feed's events, errors, usage, summary,
retained capacity and UTF-8 state with the forced scalar path, including terminal
retries and summary overflow. Keep those semantics when changing the fast path.

Selective keys reuse the existing buffered ASCII path only within remaining key
capacity and already allocated token space. Raw-token, source, decoded and work
admission remain independent; growth and boundary bytes stay scalar. This retains
the same token contents, capacity trajectory and error priority, including after
escapes, split input and long-key spill. It does not borrow keys in a way that
bypasses their capture budgets or alter the accepted geometric growth policy.

Default `JsonStream` token borrowing, capture, counters, errors, limits and WAF
verdicts remain unchanged. Selective coverage is additive: preserve the existing
test identities and compare whole, byte and mixed schedules under every budget.
The fresh supporting full WAF library run for Task 02 passed 642 tests with two
ignored; this is a dated implementation result, not a substitute for revalidation.

## Optional capture hit batches

`capture_hit_plan` compiles generation-owned boolean shadows after all required
plans, borrowing the still-live validated source arena. Ordinary matcher inventory
and original capture programs remain distinct. Source-member target/transform
identity determines candidate roots; each JSON/XML domain maps them through its
canonical StaticFact groups. Canonical peer aliases can cross target plans, so a
later overlapping ownership candidate falls back. Empty domains retain no optional
metadata. Compiler record work, source/pattern/program counts, capacities, compile
overlap and lazy request scratch are bounded before publication. All optional
state publishes atomically; budget/build fallback never suppresses required source
validation. Only pre-build peak refusal permits bounded source-order halving.

A borrowed field/profile/stage may use a shadow only when every peer is selected,
active and nonterminal at stage entry. The eligible set is frozen for that stage;
next stages recompute it, including after multiMatch terminals. Stamp wrap clears
stale selection evidence. Stage-owned cached bits establish positive existence;
original negation and LeftmostFirst extraction still own exact capture semantics,
checkpoint errors and aliases. Reference mode retains individual detection. Never
run capture extraction for misses or retain per-field predicate matrices. Constructor
scratch envelopes include optional engines even though their caches initialize
only on use; report that cost separately from observed heap and actual latency.

Directed owners are the capture-hit compiler, collector, JSON/XML replay tests and
explicit release witness under the canonical unit-test tree. Re-evaluate on source
profile, group aliasing, dispatch order, cache namespace, epoch, checkpoint, compiler
fallback or scratch-envelope changes. Synthetic timings do not establish a complete
CRS or Gateway speedup.

## Ordinary current-input scan reuse

Ordinary Accelerated boolean Matcher operands may reuse one group's existing
result bits only for consecutive immutable request-arena ranges with the same
canonical transformation plan and without multiMatch. The inline signature
retains no bytes and has zero Vec capacity. Different source ranges, profiles,
uncacheable operands and common phase entry invalidate it; the direct prefix
matcher writer also invalidates it before touching scratch. Collector stage
caches remain separate. Logical charges, negation and ordered rule/TX effects
execute normally. A marker publishes only after successful scan
and range validation. Scratch/generation ownership belongs to the transaction;
range checks alone do not authenticate arbitrary same-shaped foreign scratch.

Directed transaction tests compare physical scan counts and ordered outcomes
against Reference, and cover duplicate fields, profile switches, TX/captures,
multiMatch and prefix/phase invalidation. Re-evaluate on any new direct scratch
writer, mutable request arena or non-deterministic transformation.

Ordinary Accelerated evaluation also has a separate one-slot borrowed-transform
memo. It records only a successful complete `Cow::Borrowed` identity walk for a
checked immutable request range, a canonical nonempty transformation plan and
the exact output/work limits. A hit borrows that input and enters the original
predicate, capture and accounting path. A miss clears the previous proof before
the canonical walker; owned outputs and failed walks never publish. Empty plans,
multiMatch, Reference and non-request values bypass it; ineligible inputs and
phase/prefix entry clear it. Its inline key retains no bytes and introduces no
heap allocation. Ordinary transform work is local to each walk, and materializing
observer checkpoints have no persistent effect, so a successful equal-limit
identity proof can replace a repeated walk without changing downstream charges.

The source argument removes repeated identity scans on exact hits, not existing
allocations or a proved percentage of request latency. One-slot alternating
inputs can miss every time. Tests and benchmarks for the original memo addition
were omitted at the user's explicit request; historical scan-cache tests are not
evidence for that addition. ARCH-232 subsequently adds the directed alias/memo
coverage described below, without a benchmark or performance claim. Re-evaluate
when borrowed-result meaning, transform determinism,
request-arena mutability, limit/checkpoint semantics or memo eligibility changes.

The 2026-10-12 scoped memo implementation passed independent plan and final
source review, format, workspace compile/clippy and static repository guards.
These results apply to that implementation; cleanup reused them without rerunning
code checks.

### Immutable request-head source aliases (ARCH-232)

`RequestState::build` decodes each query field once and copies header/cookie names
once into its existing transaction-owned immutable arena. The four query
collections and paired header/cookie collections retain separate ordered records
that share only their source field's ranges. Alias records replay after the primary
collection in the original order, preserving fallible collection-metadata growth.
Equal nonempty source entries remain separately stored; empty ranges may coincide
without padding. No transport borrow survives construction, and scalar/raw/path
views retain their original decoding and ownership.

The existing projection walk computes compact physical arena reservation separately
from the original nominal alias payload. Nominal checked arithmetic and semantic
input/count/admission precedence remain unchanged. Retained preflight uses the
original record projection, while final retained bytes use nominal payload plus
actual entry metadata. Constructor scratch and later body/TX/response aggregate
admission continue to consume this nominal retained charge. A smaller physical
arena does not grant additional logical retained headroom; allocation success may
improve solely because the arena reservation requests fewer bytes.

Shared ranges allow the existing scan cache and borrowed-transform memo to reuse
eligible source aliases. They do not merge logical collection identity, selected
records, operator charges, captures, TX writes or control exclusions. The existing
profile/limit keys, successful borrowed-identity publication and mutable-source,
multiMatch, Reference, phase and prefix exclusions remain unchanged.

Private `transaction_test.rs` regressions cover literal ordered decoded records,
range sharing and source separation, natural empty ranges, compact payload length,
independent legacy nominal expectations, exact/one-less request/constructor/later
body retained admission, competing validation errors, physical scan reuse and
per-selected-value logical limits. Accelerated/Reference comparisons cover ordered
outcomes, named captures, TX and collection-specific controls. Directed memo tests
cover identity versus owned/failed transformations, changed profiles/limits,
ineligible sources and empty-phase/prefix invalidation. These are bounded local
representation/semantic checks, not allocator-count, throughput, total heap or RSS
measurements. Re-evaluate on arena mutability, new source aliases, projection or
admission order changes, or changes to either range consumer's key/eligibility.

Current raw-name selector admission and subscription dispatch reuse successful
boolean results within one collect_value call using two inline u64 words. The
name stays immutable for that call; new JSON fields and XML attributes construct
fresh state. Program IDs >=64 and Reference execution use original searches.
Work charges and program/slot bounds validation precede reuse. Explicit target
exclusions remain canonical replay work. No names, overflow allocation or field
matrix are retained. Re-evaluate on selector ownership/profile or field lifetime
changes; physical call-count tests do not establish total request latency.

## Optional necessary-prefix regex guards

Compiler publication attaches guards only after all required plans and optional
capture shadows. Proof walks each validated NFA start_pattern along mandatory
singleton byte/epsilon edges. Branches/classes/Dense stop proof; every pattern
must yield >=3 bytes. Absolute-start anchored programs decline guards. Proof
has64-pattern/256-byte/8192-operation bounds and charges visits and copies.

Native AC only rejects a negative union on >=64-byte inputs; any candidate is
confirmed by the original whole-input VM. Grouped Reference bypasses guards.
Scratch ownership validation and cleared result bits precede misses. Captures
remain exact. Final global and per-program retained/peak budgets own optional
attachment, with no subsequent required allocation. Proof frames are pre-admitted
and their fallback peaks recorded. Actual heap capacities are retained charges;
admitted failed builds charge peak only. Budget/proof/build fallback keeps original
matching and never suppresses required validation.

An additional128KiB guard-local projection covers the native prefilter's transient
64KiB two-gram table, packed buffers, growth overlap and auxiliary state. Recheck
that bound when its constants/allocation change. This explicitly accounts for
temporary storage absent from the native post-build diagnostic, without claiming
the generic AC projection was changed. Before exact lowering, boolean programs gained24 inline bytes;
capture program layout and request engine scratch are unchanged. Original-package
coverage is deliberately narrow; synthetic speedups do not establish CRS/Gateway
latency. Exact/one-less optional build, tight global admission, VM parity,
fanout and original supported package witnesses are regression gates.

## Exact simple boolean regex execution

The final optional pass proves a singleton validated NFA path with at most16
literal bytes and64 state/copy operations. Only absolute start/end looks,
singleton byte transitions and capture/single-union epsilon transitions qualify.
Bytes after end, branches, classes, other looks and exhausted proof budgets
retain the VM. Empty predicates preserve boolean existence. Proof uses a
pre-admitted256-byte envelope and charges successful or declined traversal work.

A32-byte inline execution hint selects exact substring/prefix/suffix/equality
comparison before necessary guards. Its overhead is eight bytes more than the
previous guard Vec. The original required NFA stays for strict validation and
Reference execution; there is no NFA-memory savings claim. Owner validation and
cleared physical result bits precede comparison. Group fanout, captures, actions
and replay remain canonical. Re-evaluate on NFA look semantics, supported proof
shapes, hint layout, compiler pass order, or scratch-owner changes. Directed
byte/Unicode/empty/anchor/fanout/budget tests and paired release witnesses are
regression evidence; synthetic timing does not establish Gateway throughput.

## Stage-cache membership storage

The complete generation matcher namespace includes ordinary and optional
capture-shadow engines. A namespace<=64 uses one u64 for current-stage
membership, with the separately compiled per-domain logical capacity. A larger
namespace uses the original bounded Vec, so a sparse high ID never aliases an
inline bit or causes a global-size bitmap allocation. Constructor validates
domain capacity against the complete namespace, reserves overflow fallibly and
retains actual allocator capacity. Checked shifts, full insertion failures and
post-scan/range publication prevent stale or invalid completion records.

Ordinary current-input reuse owns Single replacement storage. Collector stages
clear membership and retain the old constructor/reset work charges, including
reservation work even when inline eliminates its heap. Current production
cache storage measures24 bytes; collector heap accounting excludes inline
owners. Do not claim matcher-scratch or body retention savings from this change.
Re-evaluate on namespace, domain capacity, alias, engine-write or reset changes.
Canonical cache tests cover58/64/65 namespaces, sparse high IDs, both modes,
exact/one-less physical admission, failure non-mutation and capacity reuse.

## Completed multi-pattern optimization evidence

The completed A-H series interns selector programs (37 logical occurrences to
12 programs), shares duplicate boolean patterns (66 to 60), adds 22 optional
capture-hit engines with 72 patterns, reuses current-input/current-name results,
and lowers proven simple patterns. Current 36 ordinary plus 22 shadow engines
fit inline stage membership. Rule ordering, exact captures, Reference fallback,
work charging and optional aggregate policy remain unchanged.

The historical final comparison used CountingSystem over System in debug builds,
three warmups and 31 samples per row: 28 rows cover seven fixtures, both profiles
and both expensive-check settings. With expensive checks off, structured standard
fixtures reduced cumulative requested bytes by 2.2-2.9% and extra live-heap peaks
by 3.1-3.8%; strict fixtures reduced them by 9.4-11.7% and 11.7-13.2% respectively.
Ordered outcomes, selected TX values and multipart counters matched the baseline;
post-Drop live deltas were unchanged, not proven zero. Source bindings were
checked against the unchanged WAF implementation at cleanup. Raw process artifacts
were removed with completed tasks. These results do not measure Gateway latency,
throughput or RSS. Establish fresh Gateway performance evidence using the
[testing guide](../../../05-testing/01-integration-testing.md); the historical
performance task record is absent and its removal does not establish measurement.

Capture-hit acceleration adds generation memory and conservative admission cost:
its measured engine-plus-metadata delta was 668,878 bytes and incremental scratch
envelope 2,567,160 bytes, including empty slots; engine caches stay lazy. Final
reported generation retention was 9,221,804 bytes and conservative compile peak
1,048,458,311 bytes under the unchanged 1 GiB cap. Compiler forecasts exclude
allocator fragmentation and are not observed process memory.

Historical combined verification passed 776 library and 19 integration tests
(with existing ignored cases), workspace all-target check/clippy, both original
fixture targets, and live WafDynamic/WafExpensiveChecks scenarios. Re-run relevant
gates after implementation changes; these counts are historical, not certification
of arbitrary CRS configurations or full response-body support.
