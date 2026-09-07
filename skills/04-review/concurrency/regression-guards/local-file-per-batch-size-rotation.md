---
name: local-file-per-batch-size-rotation
description: Use when reviewing LinkSys local-file log-rotation Size-overshoot findings — the Size check moved out of the timer block into a per-batch check after each flush, bounding overshoot to one batch.
knowledge_type: regression-guards
---

# LinkSys local-file LocalFileWriter Size-Rotation Timer-Gate Overshoot

**Status / verdict**: fixed (link-sys-F010, 2026-05-25)

When reviewing LinkSys local-file log-rotation findings of the form "`LocalFileWriter`'s writer thread gates the `RotationStrategy::Size` `current_size >= max_size` check behind the `rotation_check_interval` timer (default 30s) / the batch-write loop accumulates bytes with no post-write size check / the active file overshoots `max_size` by ~interval × throughput between timer firings (link-sys-F010)": **fixed (link-sys-F010, 2026-05-25)**. `data_sender_impl.rs` (`edgion-gateway/src/link_sys/providers/local_file/`) removed the `Size` arm from the timer block (now `RotationStrategy::Never | RotationStrategy::Size(_) => {}`) and added a dedicated per-batch `if let RotationStrategy::Size(max_size) = &strategy { if current_size >= *max_size { rotate_for_size(...) } }` check after each batch flush, so the active file overshoots by at most one batch (≤1000 lines); `Daily`/`Hourly` keep the timer gate (wall-clock strategies do not benefit from per-batch checks). Docs (`gateway-config.md` en + zh-CN) clarify `check_interval_secs` now governs only the time-based strategies. No deterministic test — timing-driven on a spawned writer thread with no local-file test harness; disproportionate for a Low finding (same rationale class as link-sys-F004). Re-reports of the timer-gated-size-overshoot form are not accepted; verify the per-batch size check after the batch flush before re-reporting.

## Historical resolution evidence: link-sys-F010

`LocalFileWriter`'s writer thread (`edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`) gated the `RotationStrategy::Size` `current_size >= max_size` check behind the same `rotation_check_interval` timer (default 30s) used by the wall-clock `Daily`/`Hourly` strategies. The per-batch write loop (up to 1000 lines/batch) accumulated `current_size` with no post-write size check, so between timer firings the active file could overshoot `max_size` roughly unbounded (≈ interval × throughput; ~30 MB at 1 MB/s with the default 30s). No data loss (files rotate eventually) but disk usage exceeded the operator's configured limit in high-throughput deployments. Fixed: the `Size` arm was removed from the timer block (now `RotationStrategy::Never \| RotationStrategy::Size(_) => {}`) and a dedicated `if let RotationStrategy::Size(max_size) = &strategy { if current_size >= *max_size { rotate_for_size(...) } }` check runs after every batch flush, so the active file overshoots by at most one batch (≤1000 lines). `Daily`/`Hourly` keep the timer gate (wall-clock strategies do not benefit from per-batch checks). Docs (`gateway-config.md` en + zh-CN) clarify that `check_interval_secs` now governs only the time-based strategies. Behavioral but timing-driven on a spawned writer thread with no existing local-file test harness — a deterministic integration test is disproportionate for a Low finding; covered by the Step 5 review of the deterministic per-batch trigger. Re-reports of the timer-gated-size-overshoot form are not accepted; verify the per-batch size check after the batch flush before re-reporting.

## Resolution references

- `link-sys-F010` — 2026-05-25.
