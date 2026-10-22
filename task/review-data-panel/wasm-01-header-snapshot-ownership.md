# Wasm header snapshot and replay ownership

Status: deferred representation and admission decision. No runtime change.
Source baseline: ddf723e71.

## Source evidence

`edgion-gateway/src/plugins/http/wasm/host_abi.rs` implements guest header
replacement and addition by decoding owned name/value Strings, fallibly cloning
them for the callback's readable header snapshot, and retaining the originals in
`HeaderOp::Set` or `HeaderOp::Add`. Whole-map replacement decodes `new_pairs`,
calls `try_clone_pairs`, installs the clone in the snapshot and queues the
original pairs with the names to remove. Subsequent guest reads observe the
snapshot; later session replay still needs every queued operation in order.
Two logical views are required, but separate copies of immutable payload bytes
are not proven necessary.

`simple_header_mutation_profile` accounts for both copies. Whole-map replacement
uses `decoded_map_allocation(...).repeated(2)`, plus removal and pending-vector
profiles. Transient allocation and cumulative retained-byte/op admission are
checked before semantic mutation. Reservations can change collection capacity
before a later reservation fails; atomic rejection means unchanged semantic
header state, pending operations and counters, not unchanged allocations.

`WasmPlugin::finish_callback` takes pending operations and the local response,
releases callback-frame payloads, then applies the detached operations to the
session. A new shared owner must remain alive through this handoff without
retaining request data in an idle VM. Both standalone and attached response
headers use callback frames; request callbacks use the same ABI mutation code.

Initial snapshots are a separate cost: `runtime/session_adapter.rs` copies
text-eligible live request/response values into owned pairs. Changing this to
raw bytes or borrowing live session state would expand the present scope and
must not be bundled into the guest mutation optimization.

## Decision alternatives

- Retain the current shape and measure byte-copy/allocation costs for short and
  long names/values, repeated replacement and duplicate-heavy whole maps.
- Share immutable decoded payloads between snapshot and replay entries, while
  retaining their separate ordered structures. Compare owner allocation,
  reference-count traffic, structural sizes and detached replay lifetime against
  saved copies; sharing is not automatically cheaper for small strings.
- Evaluate a callback-local indexed owner only with a reviewed lifetime and
  release design. No unsafe self-reference, borrow of guest memory across guest
  execution, eager session mutation or operation coalescing is authorized.

Keep conservative existing admission decisions where the new representation
can be proven covered. Recalibrating profiles to admit more operations under the
same configured limit changes observable rejection boundaries and needs explicit
approval and independent accounting. Never assume old charges cover new owner
metadata, spare capacity or overlapping detached output. Byte/op limits remain
mandatory; no complete host-heap or process RSS bound is established here.

## Acceptance for a selected implementation

Preserve guest decoding and status/error precedence, read-your-writes, original
header spelling, duplicate order, replacement removal of omitted fields and
first-set/later-append replay. Preserve response transport-header protection,
best-effort writer failure behavior, read-only phases and trap/cancellation
cleanup. Guest allocator reentry must not invalidate host-to-guest snapshots.
Do not replace expected-linear deduplication with nested scans merely to remove
a temporary set, or change text eligibility while changing storage.

Exercise exact and one-byte-short admission, operation exhaustion, no partial
semantic mutation, repeated set/add/remove/replace sequences, both header maps,
mixed-case duplicates, malformed guest strings/maps, guest allocator reentry,
and detached replay after frame release. Existing tests such as
`replace_all_profiles_structure_and_keeps_first_header_spelling` and
`guest_add_header_flood_no_partial_mutation` provide starting coverage, not proof
of a new representation. Validate ordinary and attached callback lifecycles.
Measure transient and retained allocations as well as CPU before claiming a win.

Any implementation requires its own IMPL plan, frozen file scope, independent
reviews and project checks. The original documentation batch recorded source inspection only; no fresh
Rust test or performance benchmark ran in that batch.

Relevant original knowledge:
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/wasm/regression-guards/buffer-range-snapshot-before-guest-allocation.md
- skills/04-review/cpu-memory/regression-guards/wasm-abi-retention-budget.md


## Detached ReplaceAll deduplication (audit at 6625b4291)

`plugin.rs::apply_ops` consumes each ReplaceAll pair after removing omitted
headers. It skips protected response fields, constructs an owned ASCII-lowercase
String for each eligible name, and inserts it into a local HashSet. This repeats
normalization even for already lowercase names and duplicates. Empty strings
need not allocate; no per-pair heap-allocation count is established here.
The set retains unique canonical keys while consumed original names and values
are released as replay advances.

The neighboring `finish_local_response` borrows its header vector and stores
Cow keys, borrowing names without uppercase ASCII. Applying that pattern to
ReplaceAll could avoid some copies but would retain the original pair vector's
name/value storage through the loop. Large processed values would no longer be
released as early. Consuming original names into the set after replay is another
possible design, but preserving the original spelling for the session call can
require separate lookup and insertion work. Fewer copies alone do not select a
winner. Neither alternative is implemented or authorized by this audit.

This is a later, detached-replay decision, separate from sharing guest-readable
snapshots with queued operations. Compare the current consuming loop, borrowed
keys and ownership-aware replay under a separately frozen scope. Keep expected
linear deduplication and the canonical header policy; do not add nested scans,
a new case-folding/hash mechanism or precomputed per-pair metadata without its
own reviewed value and allocation accounting.

Preserve ordered removals, first-set/later-append behavior, original spelling and
best-effort writer failures. In particular, the first occurrence remains seen
even if its session write fails. Protected response fields are skipped before
seen-set insertion. Unknown map IDs produce no session writes; their current
internal deduplication is not evidence that changing map handling is authorized.
Do not coalesce duplicate fields such as Set-Cookie or change ASCII-only folding.

Measure short/long names and values, duplicate-heavy maps, mixed-case names,
protected fields and writer failures. Track unique-key capacity, remaining pairs,
processed values and detached outputs together; preserve release on errors and
unwinding and do not retain request data in pooled VMs. Existing ReplaceAll tests
cover omitted-field removal, duplicate response values and protected fields,
while a selected representation needs focused ordering/failure regressions.

Fresh audit verification passed 269 Gateway Wasm tests serially. These verify
the current implementation, not any proposed representation or performance gain.
