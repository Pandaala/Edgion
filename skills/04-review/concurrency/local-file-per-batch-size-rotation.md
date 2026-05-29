---
name: local-file-per-batch-size-rotation
description: Use when reviewing LinkSys local-file log-rotation Size-overshoot findings — the Size check moved out of the timer block into a per-batch check after each flush, bounding overshoot to one batch.
---

# LinkSys local-file LocalFileWriter Size-Rotation Timer-Gate Overshoot

**Status / verdict**: fixed (link-sys-F010, 2026-05-25)

When reviewing LinkSys local-file log-rotation findings of the form "`LocalFileWriter`'s writer thread gates the `RotationStrategy::Size` `current_size >= max_size` check behind the `rotation_check_interval` timer (default 30s) / the batch-write loop accumulates bytes with no post-write size check / the active file overshoots `max_size` by ~interval × throughput between timer firings (link-sys-F010)": **fixed (link-sys-F010, 2026-05-25)**. `data_sender_impl.rs` (`src/core/gateway/link_sys/providers/local_file/`) removed the `Size` arm from the timer block (now `RotationStrategy::Never | RotationStrategy::Size(_) => {}`) and added a dedicated per-batch `if let RotationStrategy::Size(max_size) = &strategy { if current_size >= *max_size { rotate_for_size(...) } }` check after each batch flush, so the active file overshoots by at most one batch (≤1000 lines); `Daily`/`Hourly` keep the timer gate (wall-clock strategies do not benefit from per-batch checks). Docs (`gateway-config.md` en + zh-CN) clarify `check_interval_secs` now governs only the time-based strategies. No deterministic test — timing-driven on a spawned writer thread with no local-file test harness; disproportionate for a Low finding (same rationale class as link-sys-F004). Re-reports of the timer-gated-size-overshoot form are not accepted; verify the per-batch size check after the batch flush before re-reporting.
