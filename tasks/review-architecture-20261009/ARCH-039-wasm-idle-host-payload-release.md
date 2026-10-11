---
id: ARCH-039
status: candidate
kind: architecture-debt
priority: P3
partition: PART-346
analyst: /root/analyze_conversion
independent_verification: VERIFICATION-ARCH-039.json
---

# Release host phase payloads before returning a healthy Wasm VM to its pool

## Current ownership and concrete cost

`VmLease::drop` (vm_manager.rs 1647-1669) returns a healthy, closed-context
Store<HostState> to its private idle pool without releasing phase payloads.
plugin.rs 315-354/390-395 installs a complete request-body Vec and moves it out
only on mutation. Read-only success or local denial leaves it in the Store.
finish_phase drains pending header operations/local response, but only counts
logs; snapshots/properties/logs/unchanged staged body remain. begin_phase
(host_abi.rs 327-358) resets visible state on reuse, but request_body.clear()
retains capacity. Response filtering already releases the chunk reference
explicitly (plugin.rs 534-537).

A large permitted read-only body remains allocated while the VM is idle; later
header/tick phases clear length but can keep capacity. Multiple warm VMs can keep
individual prior allocations under actual pool/request bounds. This is avoidable
idle host residency, not an unbounded-pool claim, cross-request ABI disclosure,
physical erasure claim or measured RSS effect.

## Bounded improvement and alternative

After the existing healthy/closed-context gate, release host phase payload owners
and reset phase controls before taking the pool mutex and returning the warm VM.
Replace/take request_body so its allocation releases; clear alone is insufficient.
Release header/property/log payloads and leftover phase output/callout state at
the same boundary. Preserve persistent guest configuration, private shared data,
metric identity, tick period and initialized guest instance. Fresh phase setup
continues to install the next request snapshots.

A successful-body-only cleanup misses denials, early exits, header snapshots,
logs and direct tick leases. Discarding every VM frees these owners but sacrifices
warm instance reuse. No new pool TTL/global memory manager/module cache is needed.
The cleanup does not free guest linear memory, transport buffers or independent
canonical request-body owners.

## Invariants and future verification

Poisoned/open-context cancelled leases remain discarded. Cleanup invokes no
guest code/async work; required outputs move out before drop and remain available
to the terminal writer. Body publication/coupled headers stay transactional.
Private plugin pool/shared-data/tick isolation and runtime destruction cancellation
remain; outstanding leases legitimately retain their entries. Preserve guest
configuration/persistent shared data, response streaming/current chunk observation,
callout egress/deadlines/fail modes and canonical observability.

Future tests should inspect the idle Store before another begin_phase: body
capacity/header/property/log payloads must release, while config/shared data/tick
state survive. Retain closed-context reuse, cancellation/poison/tick tests and
add unchanged-body success/local-denial wiring. Original runtime isolation,
shared-data no-eviction, hardening and ABI budget records bound active work but
require no completed host payload retention. REPORT-PART-344/345/346.json record
full primary reads and selected existing tests. Static review only, no execution,
source mutation or memory/performance measurement. Independent verification pending.
