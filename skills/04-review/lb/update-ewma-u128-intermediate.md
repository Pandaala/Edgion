---
name: update-ewma-u128-intermediate
description: Historical — use when reviewing LbBackend::update_ewma u64 overflow findings against pre-2026-09-04 code. The function was deleted with the fixed-alpha estimator; the 60s latency-cap suggestion remains rejected and its reasoning still applies to the replacement.
---

# LbBackend::update_ewma u64 Overflow

> **Superseded 2026-09-04.** `update_ewma` and the fixed-alpha blend it describes were deleted
> when the time-decayed Peak-EWMA estimator landed
> ([../../12-algorithms/lb-peak-ewma.md](../../12-algorithms/lb-peak-ewma.md)). The verdict is
> retained for two reasons: re-reports against older revisions still resolve here, and the
> **rejection of the 60 s latency cap still stands** — the replacement likewise leaves RTT
> samples unbounded (it saturates at the u32 µs field instead) and for the same reason:
> WebSocket / gRPC streaming / large uploads legitimately exceed 60 s. The overflow guidance
> itself no longer applies; the replacement's arithmetic bounds are invariants 3 and 4 of the
> skill entry. Do not follow the "verify the current `update_ewma`" instruction below — there
> is no such function.

**Status / verdict**: fixed (lb-F001, 2026-05-24)

When reviewing findings of the form "`LbBackend::update_ewma` computes `(alpha * latency_us + (100 - alpha) * current) / 100` in u64 / the EWMA multiply can overflow u64 / promote to u128 or cap the latency sample (lb-F001)": **fixed (lb-F001, 2026-05-24)**. `update_ewma` (`edgion-gateway/src/lb/backend.rs`) now performs the blend through u128 intermediates and casts back to u64. The cast is lossless: the formula is a weighted average (`alpha <= 100`) so the result is bounded by `max(latency_us, current) <= u64::MAX`, and the EWMA does NOT accumulate (each sample is a single-request elapsed time from `attempt.rs`, no running sum), so the only overflow risk was the intermediate products `alpha * latency_us` / `(100 - alpha) * current` exceeding u64 before the divide — `100 * u64::MAX` fits in u128. This matches Envoy's Peak EWMA philosophy (stores the EWMA as a wide `double`, clamps only the smoothing factor, never caps RTT samples). The issue's **primary** suggestion (`.min(60_000_000)` to cap latency at 60s) was **not adopted**: it changes real behavior for long-lived requests (WebSocket / gRPC streaming / large uploads legitimately exceed 60s) and diverges from Envoy, which leaves RTT samples unbounded. Unit regression `ewma_update_does_not_overflow_on_extreme_sample` (alpha=100 then alpha=0 with `u64::MAX / 50`, exercising both product terms). The bare overflow was physically unreachable (requires a ~5,840-year single sample), so the fix is zero-behavior-change hardening, not a live-bug fix. Re-reports of the overflow form are not accepted; the 60s-cap suggestion is specifically rejected. Verify the current `update_ewma` uses u128 intermediates before re-reporting.
