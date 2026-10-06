# Publish Bounded JSON Values and Paths

## Authoritative checkpoint (2026-10-06)

Status: Completed current fused value/path publication.

Compiled selectors and transform profiles run as values complete. Object-local duplicate identity uses reusable scoped key arenas; replacement removes provisional subtree facts. Only selected results, captures and finite named dependencies persist. Canonical source-order replay preserves chains, TX and captures. Broad dynamic demands requiring a complete field map are rejected rather than materialized.

Current implementation, verification and independent review are tracked in
[plan 16](16-json-capture-wiring-and-closure.md) and
[validation task 06](06-json-validation-and-benchmark.md).
These checkpoints supersede the historical interfaces and acceptance statuses
below. Historical measurements retain their original scope and revisions.

## Historical checkpoints (superseded design)

## Meta

| Key | Value |
|-----|-------|
| Status | in-progress (leaf implementation exists; acceptance open) |
| Priority | P1 |
| Depends on | `02-incremental-json-tokenizer.md` |

## Goal

### Current continuation boundary (2026-10-05)

The S4-S13 sections below are historical implementation evidence. The bounded
public start/feed/finish transaction API now exists and an explicit full-snapshot
Gateway adapter is implemented under task 05. Final structured-class promotion,
broader differential/performance acceptance and accurate provisional observability
remain open. This does not deliver a cache-free transport lane.

Convert tokenizer events into exact bounded predicate summaries and metadata
for eligible JSON rules without building a payload DOM. General variable views
remain on the snapshot path: arbitrary scalar values cannot be published after
their payloads have been discarded. Preserve reference
semantics with independently bounded key/subtree identities and rule-result
metadata; do not retain prior scalar payloads for eligible pure predicates.

## Work

- Maintain a bounded object/array path stack.
- Consume provisional scalar events in source order, then finalize observable
  results in complete-body reference order. Token order is not publication order.
- Define duplicate-key and array-index behavior according to the frozen
  contract.
- Reuse transaction-owned matcher scratch for eligible phrase groups.
- Release scalar token storage after eligible pure predicate subscribers consume
  it. Retain bounded key/subtree identities and result summaries until duplicate
  replacement and reference order are resolved.
- Keep captures, dynamic operands, chains, TX/control actions, and later readers
  of matched payloads on the bounded snapshot lane until exact retention and
  ordering are proven; do not infer eligibility from a scalar operator alone.
- Add a completion summary containing counts and limit outcomes only.

## Tests

- Differentially compare provisional paths and normalized scalar callbacks with
  the complete JSON body processor after duplicate replacement. Separately
  compare final predicate summaries, accounting, errors, findings and
  interventions with complete transaction execution; do not promise arbitrary
  final variable publication without payload retention.
- Cover duplicate names, empty names, nested arrays/objects, escaped names,
  scalar roots, and every chunk split.
- Assert that payload bytes never enter `Debug`, error text, or metrics.

## Exit gate

All supported JSON rule units produce the same observable results and
deterministic limit failures as the bounded complete-body reference while retaining only bounded
current-token/path state plus independently bounded key/subtree/result metadata.
No variable-size payload DOM or second request-body cache is permitted.

## Duplicate and order contract

S2 makes the original source-order/reference-order conflict explicit. Preserve
current last-key-wins semantics: a repeated object key replaces the entire old
subtree, including all provisional descendant results. Keep internal identity
as typed path segments, because literal dotted JSON keys collide with nested
paths in the existing external `json.*` naming scheme. Arrays preserve numeric
index order and append their length pseudo-value after children; object results
follow the reference key order. Number values use the reference normalization.

A bounded result tree is not constant memory independent of unique keys. Set
independent hard limits for key bytes, subtree identities, rule summaries and
work; charge before growth and terminate explicitly at a limit. No silent
truncation or late unbounded snapshot fallback is allowed. Snapshot needs are
resolved before consuming transport bytes and use only canonical BodyStorage.

No success/intervention is committed from JSON tokens before valid completion:
a later replacement or malformed suffix can invalidate provisional results.
Eligibility analysis must follow actual observer lifetimes: each rule unit
clears inherited matched variables in its staged TX clone before evaluation,
while capture slots persist in TX. Chain children and same-unit actions can
observe current matches. Even without payload observers, matched-state byte
accounting persists after a successful unit and affects later retention checks.
See task 01 for the ordered summary and deferred-error requirements.

This is the conservative implementation direction; task 01 remains open until
the exact rule subset, limits and differential proof are frozen. The lexical
API delivered by task 02 does not itself satisfy this publisher gate.

## S4 reducer design evidence

A persistent tree of every closed descendant is unnecessary for ordered
summary composition. Use an open-container stack and one reusable external
path buffer. Each open object retains decoded member keys and completed child
summaries in a fallibly allocated sorted vector. Closing the object folds its
children in decoded-key order and releases their keys/slots. Repeated keys
replace the old child's complete summary. Identity belongs to the typed parent
frame and decoded member key, never to an ambiguous dotted external path.

Arrays fold completed children immediately in numeric order and append their
length pseudo-value last. A parent object can later replace that complete array
summary. This retains unresolved object siblings rather than all closed scalar
payloads. Borrowed scalar callbacks consume the current normalized value;
number normalization continues to use the existing serde_json semantics.

Fixed-size retention summaries alone do not prove complete failure equivalence.
For example, count and byte limits can both fail, and a global byte peak does
not identify whether it occurred before the count limit. Body entry/decoded/work
limits similarly depend on the incoming counters from earlier sorted siblings.
The planned exact implementation must retain separately bounded, payload-free
ordered accounting events until it has a proven smaller representation:

- Publication events preserve node visits, path extensions and entry pushes,
  carrying lengths/depth/error tags rather than scalar bytes.
- Scalar events preserve candidate order and bounded per-rule selected/matched
  facts, costs and deferred predicate failures.
- Child sequences use arena indices; object closure links sequences in sorted
  order, arrays link in numeric order, and duplicate replacement releases the
  replaced child's sequence through a work-charged freelist walk.
- At valid EOS, publication accounting follows the existing BodyBuilder check
  order. Scheduled rule execution preserves the separate selection/operator
  and match-application passes. An unreachable rule cannot publish its error.

Predicate/final-publication errors can disappear with a replaced child; lexical
syntax errors and actual state-budget failures cannot. The fixed-size summary
algebra is an acceleration building block, not a replacement for this ordering
evidence or for transaction commit logic.

Independent limits are required for allocated capacities, live key bytes, live
member slots, accounting events, path/depth and cumulative work. Charge key
comparisons, vector shifts, growth copies, folds and discarded-event walks;
fallibly reserve only after admission and check resulting capacity. Reverse-
sorted wide objects can exhaust work even with bounded memory, so measure that
shape before claiming a speedup. No defaults are frozen by this design note;
calibrate them against the corpus and runtime budget before enabling the lane.
Terminal failure and abandonment must release all owned capacity.

## S5 implementation boundary and oracle

The reducer foundation accepts explicit independent limits; it does not freeze
Gateway configuration defaults. It exposes surviving ordered events only after
a successful lexical completion. Callback results are provisional fixed-size
metadata. A revocable predicate error belongs in that metadata; returning a
sink error terminates parsing and cannot be undone by a later duplicate key.

The complete-body adapter has an additional ordering obligation: source-byte
work and initial body-length/processor metadata limits precede JSON parsing;
malformed syntax precedes surviving traversal failures. The standalone reducer
reports lexical errors directly and is not yet that adapter. Empty HTTP body
acceptance also remains an adapter concern. Independent lexical/reducer depth,
path and memory limits are distinct from final surviving-publication limits.

The implemented independent oracle keeps copied callback payloads only in tests,
identifies them with a numeric result, and replays actual surviving events
through the existing BodyBuilder. It compares all publication records, logical
retained bytes, and competing limit failures against complete-body processing.
This proves metadata ordering without adding a production payload cache.

S6 extracts those checks into the internal `BodyAccounting` owner and exposes
`validate_json_publication` for trusted successfully completed `JsonDocument`
metadata. It shares entry/name/decoded/work and finish-time arithmetic with
materializing processors; multipart counters and XML-specific rules stay local.
The returned summary counts logical publication bytes and descriptors, not
reducer allocation capacity. No scalar payload is reconstructed. Existing
materialization no longer scans entries again for ARGS size or retained bytes.

Raw overrides and head-size overflow are compared with materialization. Literal
cross-format baselines independently pin raw, URL encoding, JSON, XML,
multipart, fallback and empty HTTP body accounting. Active, failed and abandoned
documents are rejected before body-budget checks. The malformed/empty HTTP
adapter and eligible-rule subscriber/replay remain pending; S6 does not enable
streaming rule execution or change the conservative execution classifier.


## S7 body adapter boundary

`JsonBodyAdapter` now owns bounded source admission and EOS accounting. It
accepts empty HTTP bodies, defers ordinary malformed syntax until final length
is known, and reuses canonical accounting for valid JSON and detection-only
fallback. The raw override remains three-state. Fallback preserves the original
malformed flag and can itself fail its fresh publication budget. Summaries
represent logical publication costs; no raw/scalar payload cache is created.

The adapter's private tokenizer mode also enforces the reference serde_json
parser's 127-container nesting limit, even in overwritten source subtrees.
Standalone `JsonDocument` and `validate_json_publication` still permit separately
configured syntax limits: the latter validates publication accounting only.
Full reference-body syntax/precedence callers must use the body adapter.

Source admission checks the complete chunk against the independent lexical
source bound before dispatch and remains active during malformed length-only
drain. Resource/internal/subscriber failures are distinct immediate terminal
errors; they never become detection-only malformed fallback. Within those
streaming budgets, canonical byte/work/prelude/syntax/traversal/finish errors
retain complete-body ordering. Events are inaccessible until valid successful
completion; failure and abandonment discard document capacity.

S7 still does not install variables or execute rules. Eligible subscriber facts,
query-before-body execution, deferred operator errors and virtual TX retention
remain the next transaction-facing work. No classifier or Gateway lane changes.


## S8 internal subscriber and replay bridge

The internal `transaction::json_rules` plan admits complete Phase-2 candidate
sets only: one member/target, ARGS or ARGS_NAMES, all/literal selector, static
noncapture phrase operator, effective empty/lowercase transformations and static
metadata/pass/deny/logging actions. Global controls and dynamic templates reject
the plan. This is deliberately not the final hybrid generation classifier.

Sessions borrow the same generation's transaction matcher scratch and consume
canonical query pairs before the JsonBodyAdapter. Fixed-size facts retain lengths,
match bits and deferred transformation-limit failures. An independently bounded
append-only arena charges all provisional facts, including overwritten subtrees.
Its count, allocated-capacity and speculative-work caps have no frozen defaults.
Temporary transformation buffers use the existing runtime output/work limits.
Shared matcher execution, operator-work and runtime-template length checks serve
both the existing evaluator and this standalone replay.

Replay completes selection before deferred predicate checks, then performs all
operator/match collection checks before ordered TX count/byte checks and static
metadata validation. The caller supplies TX size after clearing inherited matches;
nonmatch does not instruct the caller to commit that cleared state. Whole-request
retention/collection admission, scheduled unit counters, findings, intervention
and transaction commit are not implemented here. Body/head configuration must
match the canonical request/runtime, and scratch must match the plan generation.

This bridge remains internal and is not scheduled. Differential tests compare
actual evaluate_unit behavior, including query order, negation, transform resets,
duplicate removal, competing limits, static metadata and nonzero TX base. Future
integration must enforce the omitted global checks and preserve prior committed
match charges across nonmatching units before any classifier/Gateway promotion.


## S9 internal transaction integration

The internal `execute_json_body_inputs` seam now derives canonical query/body
policy from the transaction, consumes borrowed incremental inputs, detaches
owned completed facts and runs the existing Phase-2 schedule. It does not replace
findings, omission counters, denial/detection behavior or phase advancement with
a second scheduler. This synchronous iterator is a proof seam, not the future
Gateway callback interface; callers must not aggregate chunks to invoke it.

Request publication explicitly distinguishes materialized entries from metadata.
Both use one atomic collection-count/retained-byte admission helper. Metadata
keeps the reference logical publication charge and body-error flag without
pretending to expose missing values. The plan rejects later-phase body readers
using the canonical compiler collection dependency predicate. Runtime lookup
also fails closed for unavailable metadata-only collections.

Virtual matched state stores logical match count and payload charge. Clone,
clear and recount preserve ordinary TX/capture state; successful replay uses
normal commit checks, while nonmatch retains the old committed match charge.
Safe later independent MATCHED_VARS readers see the normal cleared state, and
chains see their own new materialized matches. Response admission continues to
include reference logical body and TX charge.

Completed facts use private cold storage with a 0/1 invariant, reserved fallibly
before publication. Ordinary transactions retain only the empty vector header,
not an inline completed parser owner. Phase-2 runtime errors preserve facts and
prior commits for normal retries; success/denial releases the cold allocation.
Actual fact/document/plan capacity bounds remain independent of reference logical
transaction accounting. Scoped collection and attachment bind the facts to the
originating request; do not expose arbitrary detached-fact binding as a public API.

Gateway wiring, persistent asynchronous collection, production default budgets,
performance measurements and execution-class promotion remain pending. The
current conservative whole-generation restrictions are route eligibility checks,
not new configuration rejection rules.

## S10 persistent internal collection

The transaction now owns its bounded collector across calls. Internal start,
feed and finish methods separate whole-source admission, borrowed chunk input,
and atomic publication plus Phase-2 execution. Complete seals provisional state;
finish publishes it. The synchronous proof helper delegates to these operations,
so trailing input still fails before publication. This is not Gateway integration.

Each feed temporarily borrows the transaction generation and existing matcher
scratch. Subscriber descriptors and the shared collection core own no source,
generation or scratch references. There is no accumulated chunk list or second
scratch allocation. A cold 0/1 enum owner holds collecting or completed state;
completion reuses its capacity when attaching retry facts after publication.

Provisional failure frees the cold owner and arenas, retains a small sticky error,
and blocks competing materialized, prefix and raw body APIs. Rejected duplicate
start preserves active collection. Only admitted start with a NEW complete source
may reset a previous failure; restarting on the remainder of a failed transport
is forbidden. Rejected restart admission preserves the existing failure. Input
or finish misuse after publication cannot undo publication or discard retry facts.
Scheduled execution errors retain the ordinary retry contract. Success releases
the cold capacity; transaction destruction owns cancellation cleanup.

Production plan caching, budget calibration, mixed-rule execution and the canonical
pre-dispatch Gateway capture observer remain pending. No public API, configuration
field or structured execution-class promotion is included in this batch.

## S11 generation-owned subscriber plan

The compiler now prepares immutable JSON capability/descriptors after resolved
rule units, targets, actions and effective transformations are finalized. It
checks the whole generation for eligibility before allocating descriptors.
Unsupported remains a valid ordinary configuration and is distinct from an
eligible empty plan. Compiler allocation and budget failures remain errors.

Descriptor capacity is charged to the existing compiler-owned, generation and
compilation-peak limits. Allocation uses the canonical fallible reservation seam;
actual capacity beyond the requested record count is also charged. Publication
refreshes the generation accounting after plan construction.

Request initialization borrows this plan and checks subscriber count and shared
allocated-capacity bytes against JsonPlanLimits. The byte cap now admits an
immutable shared plan rather than bounding a new per-request descriptor vector.
Unsupported is returned before request cap errors. Collecting and completed state
no longer own subscriber vectors; each feed/replay resolves the immutable slice
through its transaction generation. No request-level descriptor allocation, full
eligibility scan or additional generation Arc clone remains in this path.

This change removes identifiable repeated work; throughput has not been measured.
Production budget calibration, mixed-rule support and pre-dispatch Gateway capture
integration remain pending, and the compiler execution class is not promoted.

## S12 mixed ordinary and JSON Phase-2 execution

The immutable plan now emits subscribers only for eligible JSON units. Units
whose every member is body-independent according to the canonical dependency
predicate retain their ordinary evaluator and schedule position. This permits
static header/URI/query/TX predicates, targetless actions, captures, wholly
body-independent chains, static TX mutation and existing skip/skipAfter behavior.
Descriptor limits count JSON subscribers, not ordinary units.

A reached descriptor uses bounded JSON replay; a missing descriptor requires an
explicit all-member body-independence proof before ordinary evaluation. A missing
body subscriber fails closed. The existing scheduler remains responsible for
rule order, skips, findings, limits and enforcement/detection decisions.

Ordinary matches clear and replace prior virtual matched charges; nonmatches
retain committed charges. Ordinary captures/TX writes feed subsequent JSON replay.
Chain children observe their own staged materialized matches. Scheduled errors
retain completed facts and prior commits; retry starts the original schedule
again, including re-execution of preceding ordinary actions.

Global controls and dynamic templates remain excluded. Mixed chains with any
body-dependent member, unsupported body consumers and later-phase body readers
remain ineligible. No Gateway activation or execution-class promotion is included.

## S13 dependency-based ordinary template admission

Dynamic templates no longer make the entire generation ineligible. Phase-1
macros execute before body publication, where ARGS contains query values. Every
ordinary post-Phase-1 unit must pass the canonical body-dependency predicate for
all members; later-phase checks now include operator/action templates and setvar
keys/values rather than only matching targets. Direct ARGS macros after Phase 1
remain ineligible.

Ordinary dynamic operators, TX/capture copies, setvar keys/values and metadata use
the existing evaluator, expansion limits and staged commit behavior. Inherited
matched state is cleared before evaluating each ordinary unit; actions and chain
children observe their own materialized matches. JSON subscribers cannot capture
or assign TX, so they cannot inject unavailable body bytes into ordinary TX state.

Every JSON subscriber operator/action template remains static. Its replay state
contains logical matched charges rather than matched payload bytes, so even a
non-body macro in subscriber msg/logdata is deliberately rejected. Global ctl
exclusion remains. No runtime macro implementation or Gateway activation changes.
