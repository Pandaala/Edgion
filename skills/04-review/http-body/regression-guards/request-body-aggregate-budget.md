---
name: request-body-aggregate-budget
description: Review process-wide request-body storage admission and payload ownership across capture, mutation, replay, snapshots, and late I/O.
knowledge_type: regression-guards
---

# Request-body storage has a shared retained-payload budget

## Conclusion and scope

`request_body/budget.rs` owns one startup-only process budget configured by
`gateway.request_body_budget`: 256 MiB of storage-owned payload capacity, 2 GiB
of spill bytes, 1024 admitted captures and 1024 open spill files by default.
All EGC generations/listeners share it. Configuration validation rejects zero,
invalid sizes and unknown fields; repeat initialization fails rather than
publishing a second allowance. Library-only consumers receive finite defaults.
Reservation is immediate CAS admission, with no capacity waiter queue.

Per-request caps, EGC hot reload and the separate 32-operation storage I/O lane
remain independent. A per-request cap violation is 413; shared capacity refusal
is `BodyReadError::CapacityExceeded`, HTTP 503, and canonical access-log
`BodyCaptureCapacityExceeded`. Main-request consumers fail closed irrespective
of ordinary fail-open/error-policy settings. An already detached best-effort
request mirror aborts its own copy; it cannot change the main request outcome.

## Ownership and accounting

Memory is reserved before creating exact addressable payload buffers. Explicit
geometric capture growth reserves the replacement while the old buffer remains
charged; spill transitions retain both memory and disk reservations. Opaque
incoming `Bytes` may retain a much larger allocation than their visible length,
so storage copies retained ingress/mutation bytes into admitted owned payloads.
The original caller/transport allocation is not retained by this storage owner.

`Bytes::from_owner` transfers memory and capture leases to published payloads:
clones and slices share one charge, and the last retained reference releases it.
This includes initial capture, mutation payloads, materialization/cache results,
range reads, isolated snapshot memory, and disk replay/snapshot outputs. Old
snapshot generations keep their backing owners. A failed mutation leaves the
published generation intact. Known final mutation lengths are checked before shared capacity admission,
preserving the per-request 413 distinction. Literal replacement first admits its
replacement input before asynchronous scanning, because the final match count
is not yet known; capacity exhaustion during that operation can precede its
final physical-length check or reject a replacement that would ultimately match
nothing. Opaque mutation inputs are explicitly released
before asynchronous scan/materialization waits.

Each `BudgetedFile` closes its file before returning disk/file credits. Append
reserves its full attempted growth into that shared file owner before the
syscall; partial failure retains this conservative reservation until the file
closes. Blocking jobs retain their file/input/output owners after cancellation
or timeout, so request drop cannot return credits for resources still in use.
Storage-owned raw disk scan buffers are also admitted. Existing storage I/O
permits/deadlines and H1 cleanup/H2 stream isolation remain in force.

## Limits and tradeoffs

These counters bound addressable payload capacity, reserved spill bytes and
resource counts, not total heap/RSS. Allocator metadata/rounding, socket and
transport allocations, OS page cache, arbitrary scanner/plugin outputs, parsed
objects and metadata allocations are excluded. `MAX_BODY_SEGMENTS` bounds one
snapshot, not the number of externally retained historical generations. Capture
slots remain held by detached payload owners, but do not establish a total
snapshot-metadata bound. Storage ownership isolation adds copies at opaque
incoming/mutation boundaries; shared slices within storage still share credits.
Disk reservations are logical written byte bounds, not filesystem block usage
or a guarantee that external processes leave free space. Startup-only limits
avoid old/new hot-reload budgets multiplying the process allowance.

## Verification and re-evaluation

Targeted tests live in `tests/unit/request_body/budget_test.rs`,
`tests/unit/request_body/storage_test/budget_tests.rs`, storage I/O tests,
and session-adapter/body-gate/DSL/Wasm tests. They cover shared admission,
release/reuse, spill overlap, memory/disk/file/capture limits, exported owners,
old snapshots, mutation atomicity, materialization, late/partial I/O, and
protocol/error-policy boundaries. Executed results are reported by the fixing
workflow; a listed test is not a claim that all integration topologies ran.

Fresh verification on 2026-10-27 against HEAD `f208c5e8975b6f990302e72e3c94775c1ece39dc`
plus this change passed workspace all-targets check and Clippy (existing warnings
in unchanged files), formatting, agent-doc consistency, SSA force, metrics
inventory, Pingora source policy, Gateway API transform contract, unit-test
layout, and Gateway tracing-boundary guards. The rebuilt Gateway test binary
passed six serial filtered runs: `request_body` (216), `capacity` (70),
`plugins::runtime::session_adapter::` (187), `plugins::runtime::body_gate::`
(32), `plugins::http::dsl::plugin::` (11), and
`plugins::http::wasm::plugin::` (45). These filters overlap. The DSL leaf's
`error::tests` also passed all three error-display tests. Serial execution
avoids the existing process-wide Wasm preparation admission limit causing
unrelated fixture-load contention. Independent source/documentation review
passed. No full workspace test suite or external integration topology ran;
H1/H2 protocol coverage used the existing in-process duplex transport harness.

Re-evaluate if storage adopts another allocation type, a new body API exports
an owned copy, the budget becomes reloadable, the file backend changes, or
error consumers stop preserving typed capacity failures.

## Resolution reference

- `edgion-request-body-capture-lacks-aggregate-budget`: aggregate retained-storage
  admission gap; Edgion baseline `6069963764e2435aaa834aaa8aa56028b0dec67a`.
- Cargo.lock selects Pingora `1aaa45cc70430bc782990bc20ddee3fef36d6b99` on
  `edgion_0.9.0_v1`; this fix does not change the dependency or protocol behavior.
