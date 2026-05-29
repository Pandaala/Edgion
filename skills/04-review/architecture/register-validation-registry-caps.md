---
name: register-validation-registry-caps
description: Use when reviewing federation RegisterRequest / ControllerRegistry / ResourceAggregator findings of the form "unbounded controller_id growth / memory amplification / SQLite explosion / missing TTL on offline rows".
---

# Federation Register Validation & Registry Capacity Caps (common-center-03)

**Status / verdict**: fixed (common-center-03, 2026-05-16)

When reviewing federation `RegisterRequest` / `ControllerRegistry` / `ResourceAggregator` findings of the form "unbounded controller_id growth / memory amplification / SQLite explosion / missing TTL on offline rows": **fixed (common-center-03, 2026-05-16)**.

`fed_sync::server::sync` now runs `validate_register_req` (controller_id non-empty + ≤ 253 B + no control chars; cluster ≤ 63 B + no control chars; env / tag / supported_kinds ≤ 32 items × 63 B each + no control chars) and `registry_capacity_exceeded` (cap 10_000; reconnect of a known id is always allowed) before any state mutation.

## Source paths

- `src/core/center/fed_sync/server/mod.rs` — `fed_sync::server::sync`, `validate_register_req`, `registry_capacity_exceeded`

## Related decisions

The retention rule in `cpu-memory/not-a-performance-issue.md` (now split into `cpu-memory/cases/`) still rejects TTL/eviction-sweep findings.

## Re-report guard

If a finding claims the boundary checks are missing, verify the current `src/core/center/fed_sync/server/mod.rs` before re-reporting; if a finding proposes loosening or removing the caps, reject — they are the defence-in-depth that complements `common-center-01`'s mTLS work.
