---
name: wasm-oci-purge-interval-zero-clamp
description: Use when reviewing findings that a zero/sub-second wasm_oci_cache_purge_interval_secs busy-loops the OCI disk-cache purge task; covers the startup-clamp fix.
status: fixed
finding_id: contract-03
closed: 2026-07-18
---

# Zero Wasm OCI purge interval busy loop — FIXED (clamped to a 1s floor)

## Conclusion

`wasm_oci_cache_purge_interval_secs` deserializes as an unrestricted `u64`.
Setting it to `0` produced `Duration::ZERO`, so `ensure_purge_task`'s loop
(`sleep(cfg.purge_interval).await` → `spawn_blocking(purge_once)`) spun with no
pacing, resubmitting blocking directory scans for the gateway's lifetime. This
is **fixed** by a startup clamp to a documented `MIN_PURGE_INTERVAL` (1 second),
plus a defensive `.max(MIN_PURGE_INTERVAL)` in the loop.

Re-reports of the "zero interval busy-loops" / "no clamp like the pull timeout"
form are **not accepted** — verify the clamp is present before re-filing.

## Core Rationale

**1. The clamp lives at the config install point.** `configure_oci_disk_cache`
(`edgion-gateway/src/plugins/http/wasm/oci/cache.rs`) runs the installed
`purge_interval` through `clamp_purge_interval`, which lifts any value below
1s up to 1s and WARNs once. This mirrors the sibling precedent
`configure_oci_pull`, which already clamps a zero pull timeout/attempts to 1
with `.max(1)`. `0` carries **no** "disable" meaning (unlike
`wasm_oci_cache_max_bytes: 0` = uncapped, or an empty `wasm_oci_cache_dir` =
disk cache off).

**2. Defense in depth in the loop.** `ensure_purge_task` sleeps for
`cfg.purge_interval.max(MIN_PURGE_INTERVAL)` so a directly-constructed
`OciDiskCacheConfig { purge_interval: Duration::ZERO, .. }` (a test or a future
caller that bypasses `configure_oci_disk_cache`) still cannot busy-loop. The
loop does not re-WARN each iteration — the single install-time WARN is enough.

**3. Parse stays lenient; the guard is a startup clamp, not admission.** The
YAML still parses `0` as `0` (config test asserts this); rejection/normalization
happens at startup, consistent with the rest of the gateway-local numeric knobs.

## Fix Suggestions Not Accepted

- "Reject a zero interval at deserialization / fail startup" — the project clamps
  these gateway-local numeric knobs rather than hard-failing (see
  `configure_oci_pull`); a WARN + clamp keeps a misconfigured gateway serving.
- "Give `0` a disable-the-purge-task meaning" — the size budget already bounds
  the directory on every `store`, but the TTL sweep is still needed; there is no
  operator need for a disable toggle, and `0`-as-disable would collide with the
  clamp intuition operators get from the pull knobs.
- "Warn on every loop iteration" — noise; the install-time WARN already tells the
  operator once.

## Re-evaluation Triggers

Re-open only if:

- A legitimate use case needs sub-second purge sweeps (none today — default is 1h,
  TTL default 24h).
- The purge task is refactored so `configure_oci_disk_cache` is no longer the
  single install point (then re-audit where the clamp must live).

## Reference Cases

- Finding contract-03 (`tasks/review-wasm-issues/`), closed 2026-07-18.
- Source: `edgion-gateway/src/plugins/http/wasm/oci/cache.rs`
  (`MIN_PURGE_INTERVAL`, `clamp_purge_interval`, `configure_oci_disk_cache`,
  `ensure_purge_task`); precedent `configure_oci_pull` in `oci/mod.rs`.
- Config doc: `skills/02-features/02-config/01-gateway-config.md`
  (`wasm_oci_cache_purge_interval_secs` row).
- Regression test: `purge_interval_below_floor_is_clamped` (cache.rs).
