# Wasm shared-data read snapshots

Status: deferred ownership and accounting decision. No runtime change.
Source baseline: ba350c1f3.

## Source evidence

`edgion-gateway/src/plugins/http/wasm/shared_data.rs` stores each value as
`(Vec<u8>, u32)` in a DashMap keyed by String. `SharedData::get` clones that pair
while holding its read guard, then returns the detached value and CAS token.
The copy provides one coherent snapshot and releases the shard before guest code.

In `host_abi.rs`, GET_SHARED_DATA checks decoded key length plus
MAX_SHARED_VALUE_BYTES against the transient limit before decoding the key and
reading the value. This is a per-call preflight, not an aggregate reservation
across recursively nested guest calls. `write_to_guest` invokes the guest allocator
before copying the returned value; that allocator can reenter imports. The CAS
returned afterward must still describe the same snapshot. Borrowing the map value
or holding its guard across this guest call would invalidate the ownership and
reentry contract.

Immutable shared payload ownership could replace the GET payload copy with a
handle clone. It may reduce lock-held allocation/copying for large values, but
adds owner metadata and reference-count operations and changes allocation layout.
The current Vec snapshot already survives replacement/deletion. Sharing instead
retains the original allocation until the final reader releases it, so neither
shape makes current map membership a complete live-memory accounting boundary.

## Decision alternatives

- Keep the current independent Vec snapshots. Measure GET size/frequency and
  shard contention before selecting a representation change.
- Compare Bytes or another canonical immutable owner for stored and returned
  values. Measure construction, replacement, deletion and reader costs, including
  tiny values where reference counting may cost more than copying.
- Retain the existing admission policy only with a proof covering the new owner
  metadata and overlapping lifetimes. Any new policy for detached snapshots or
  recursive GET needs explicit scope and admission/error decisions, not an
  assumption that the per-store cap already covers it.

The 64 KiB value cap, 4 MiB per-store cap and 128-byte entry surcharge count the
current map contents under the existing policy. The byte gauge aggregates these
store contributions across live stores sharing a label. Neither is a claim about
all detached snapshots, allocator capacity, total host heap or process RSS. Do not
silently reinterpret the gauge as exact physical live memory or add a second
metric registry. Store destruction must withdraw only its own contribution.

## Acceptance for a selected implementation

Preserve atomic value/CAS snapshots, CAS-zero unconditional writes, nonzero
store-wide wrapping tokens, stale-token rejection after delete/recreate, and
unchanged tokens/accounting on rejected writes. Preserve checked cross-shard growth
reservation and rollback, shrink/delete accounting, capacity limits, error/status
precedence and null-delete versus empty-write semantics.

Exercise a guest allocator that overwrites or deletes the requested shared key
while the outer GET is in flight: returned bytes and CAS must remain paired with
the original read. Cover nested GET, failed guest allocation, cancellation and
release of the last reader after deletion or store retirement. No DashMap guard
may span guest execution. Compare peak/live allocation and lock duration as well
as throughput; fewer copies alone do not prove lower memory use.

Existing shared-data tests cover CAS, token wrap, concurrent growth admission,
rollback, rejected mutations, tiny-key accounting and gauge lifetime. The ABI
`shared_data_preflight_preserves_ranges_empty_writes_and_cas` regression supplies
range/status coverage. These are starting evidence, not acceptance of a new
shared-owner representation.

Keep process-local, configured-plugin-generation ownership: no LRU/TTL eviction,
Redis backing, cross-rebuild identity or restart persistence is authorized.
This task is separate from wasm-01 header snapshot/replay sharing and wasm-02
map decoding metadata. Any implementation needs a frozen scope, IMPL plan,
independent reviews and project checks after the user selects it.

Relevant original knowledge:
- skills/04-review/wasm/regression-guards/shared-data-gauge-live-stores.md
- skills/04-review/cpu-memory/intentional-design/wasm-shared-data-no-eviction-no-cluster-kv.md
- skills/04-review/wasm/regression-guards/buffer-range-snapshot-before-guest-allocation.md
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
