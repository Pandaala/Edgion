---
name: wasm-abi-retention-budget
description: Use when reviewing wasm host-ABI memory-amplification findings; covers the per-phase retained-byte/op budget + guest linear-memory cap that bound a single hook's host allocation.
status: fixed
finding_id: runtime-05
closed: 2026-07-18
knowledge_type: regression-guards
---

# Wasm guest amplifying host memory via unbounded retained ABI state — FIXED

## Conclusion

`read_guest` bounds each individual copy to guest linear-memory size, but nothing
bounded the **sum** a guest retains across many host calls in one hook. A guest could
reuse one small buffer and, in a fast loop, append arbitrarily many owned copies to
`pending_ops` / `log_lines` / `properties` until `begin_phase` cleared them after the
hook — driving the host toward OOM. Epoch interruption bounds execution *time*, not
allocation, so many successful host calls allocate between epoch checks.

**Fixed 2026-07-18** with two host-side limits (`host_abi.rs`):

- **Per-phase ABI retention budget** — `HostState::charge_abi(payload)` charges
  `payload + ABI_OP_OVERHEAD_BYTES` (64 B) against `retained_bytes`/`retained_ops`,
  capped at `MAX_RETAINED_ABI_BYTES` (32 MiB) / `MAX_RETAINED_ABI_OPS` (200k). Charged
  **before** each mutation in `proxy_log`, `proxy_add/replace/remove_header_map_value`,
  `proxy_set_header_map_pairs`, `proxy_set_property`; on exhaustion it returns
  `STATUS_INTERNAL_FAILURE` with **no** partial mutation. Reset in `begin_phase`.
- **Guest linear-memory cap** — `impl ResourceLimiter for HostState`
  (`MAX_GUEST_MEMORY_BYTES` = 128 MiB, tables 100k), installed via `store.limiter(...)`
  in `WasmVmManager::new_vm`. Bounds the guest's own footprint and thus the size of any
  single value handed to a host call, since the pooling allocator's default per-instance
  memory cap is effectively the full wasm32 4 GiB.

Fix suggestions of the form "there is no aggregate ABI budget so a guest can OOM the
host", "add a Store `ResourceLimiter`", or "epoch timeout is a time bound not an
allocation bound" are **not accepted** — all three are now implemented.

## Core Rationale

**1. Same threat model the codebase already defends, one level up.** `read_guest`
already refuses a blind `vec![0u8; len]` to stop per-call OOM; callouts
(`MAX_CALLOUTS_PER_REQUEST` = 8) and shared data (64 KiB/value, 4 MiB/module) already
cap guest-driven host growth. General ABI accumulation was the one uncapped path; the
budget closes it consistently.

**2. Append-only accumulators are the amplifier; replacement state is not.**
`pending_ops` grows on every header mutation (even a `replace`, which pushes a retained
`Set`), `log_lines` on every `proxy_log`, `properties` on every distinct key — these are
charged. `local_response` is replacement state (a single Option overwritten in place), so
its peak is one body bounded by the `ResourceLimiter`-capped guest memory; it is
deliberately **not** charged against the accumulation budget. Header snapshots grow 1:1
with charged ops, so charging the op bounds the snapshot too.

**3. Charge-before-mutate gives atomic rejection.** `charge_abi` returns `Err` before
touching any counter or collection, so a rejected op leaves no partial header mutation —
verified by `guest_add_header_flood_no_partial_mutation` (snapshot and `pending_ops`
stay in lockstep).

**4. The budget, not epoch timeout, is the controlling bound.** The adversarial unit
guests run under an engine with **no** epoch interruption, so the only thing that stops
the accumulation loop is the byte/op budget (`guest_log_flood_stopped_by_byte_budget`,
`guest_add_header_flood_no_partial_mutation`). This is exactly the "max timeout" case the
finding asked for.

## Explicitly rejected alternatives

- "The `ResourceLimiter` alone fixes it" — no: it bounds guest linear memory, not the
  host-side accumulation across calls (a guest reuses one small buffer). The byte/op
  budget is the primary fix; the limiter is defense in depth.
- "Make the limits operator-tunable config" — not accepted as a narrow fix; the caps are
  internal safety limits far above any legitimate working set, mirroring the hardcoded
  shared-data / callout caps.
- "Charge `local_response` too" — unnecessary: it is replacement state bounded by the
  guest-memory cap, not an accumulator.

## Re-evaluation triggers

- A legitimate guest workload that genuinely needs > 32 MiB retained ABI state or
  > 200k ops in a single hook (revisit the constants — but this is pathological for a
  proxy plugin).
- proxy-wasm gains a normative host-allocation accounting model.

## Reference cases

- Sibling wasm memory finding (shared data): [cases/wasm-shared-data-no-eviction-no-cluster-kv.md](../intentional-design/wasm-shared-data-no-eviction-no-cluster-kv.md).
- Code: `edgion-gateway/src/plugins/http/wasm/host_abi.rs` (`charge_abi`,
  `MAX_RETAINED_ABI_BYTES`, `impl ResourceLimiter for HostState`),
  `vm_manager.rs::new_vm` (`store.limiter`).
- Tests: `charge_abi_rejects_over_byte_budget_atomically`,
  `charge_abi_rejects_over_op_budget`, `guest_log_flood_stopped_by_byte_budget`,
  `guest_add_header_flood_no_partial_mutation` (all in `host_abi.rs` `tests`).

## Resolution references

- `runtime-05` — 2026-07-18.
