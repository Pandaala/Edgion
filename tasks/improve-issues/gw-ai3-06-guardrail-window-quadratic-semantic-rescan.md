# gw-ai3-06: Guardrail re-decodes the entire semantic window after every SSE record — O(R²) JSON-string decoding per window on the streaming hot path

- **Category**: best-practice
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/guards/semantic/response.rs:450-660` (`process_batch`: `try_retain(record)` then `self.window.stats()` for every record)
  - `edgion-gateway/src/sse/window.rs:396-409` (`RawWindow::stats` → `self.scan(0)`)
  - `edgion-gateway/src/sse/window.rs:745-760` (`scan`: `token_scratch` over all fragments, then `visit_bucket` over every bucket/fragment)
  - `edgion-gateway/src/sse/window.rs:708-744` (`token_scratch`, `visit_bucket`: copies each fragment's ranges into scratch and JSON-decodes it)
  - Default window: `edgion-resources/src/resources/edgion_plugins/plugin_configs/guardrail.rs:22` (`windowBytes` 4096)

## Summary

To decide whether the current window has reached `windowBytes` decoded semantic bytes, the
Guardrail processor calls `RawWindow::stats()` after retaining each SSE record. `stats()` does not
maintain a running count; it rescans every fragment retained so far, copying each token's raw bytes
into scratch and decoding the JSON string. LLM streams deliver ~1 token (~4 bytes) per event, so a
default 4 KiB window holds ~1,000 records and costs ~1,000·1,001/2 ≈ 500k fragment decodes before a
single decision is made. A 4,000-token answer therefore performs ~2 million string decodes per
stream, inside the async response-body callback that also gates backpressure.

## Current behavior (evidence)

- `process_batch` loop (`response.rs:450-660`): for each record → `try_retain` → after success,
  `let Ok(stats) = self.window.stats()` → `should_decide = stats.current_semantic_bytes >= self.window_bytes`.
- `stats()` (`window.rs:396-409`) → `scan(0)` (`window.rs:745`): `token_scratch` iterates all fragments
  to find the max length, then for each bucket `visit_bucket` iterates *all* fragments,
  `for_each_range` copies their bytes and `visit_json_string` decodes them, summing lengths.
- The retained-fragment count only resets when a window resolves (`complete_generation`), so cost
  per record grows linearly within a window → quadratic per window. Window size is capped by
  `MAX_EVENTS = 1024` and `MAX_FRAGMENTS_PER_BUCKET = 1024`, which bounds — but sets — the
  quadratic at ~10^6 operations per window.
- The knowledge page `skills/04-review/sse/regression-guards/window-capacity-admission.md` explicitly
  states "No claim is made that all semantic scanning across successive stats calls is amortized
  linear", i.e. the cost is known but unaddressed.

## Industry comparison

Streaming moderation built on Envoy ext_proc and LLM-guard style output scanners
keep a running byte/character counter per buffer and only materialize text when a decision is
needed (exactly once per window), giving O(window) work per decision.

## Impact

Roughly 50-200 ms of extra CPU per long guarded response (500k-2M small JSON-string decodes (`visit_json_string`) plus scratch
copies), multiplied by concurrent streams; at a few hundred concurrent guarded streams this is
whole cores spent on bookkeeping, and per-chunk latency (TTFT and inter-token latency seen by the
client) grows with window position.

## Suggested direction

- Maintain `current_semantic_bytes` incrementally: decode (or compute the decoded length of) each
  fragment once at `try_retain` time and add it to a per-bucket counter; `stats()` then becomes O(1).
  Decoded length of a JSON string token can be computed without allocation in one pass.
- Materialize text only in `snapshot_all()` / `preview_semantic_replace()`, i.e. once per decision.
- Related cheap fix in the same callback: `output_buffer()` allocates a fresh
  `BytesMut::with_capacity(~2.06 MiB)` on every callback and freezes it into the forwarded chunk
  (`response.rs:125-130`), pinning a 2 MiB allocation per in-flight token chunk; size it to the bytes
  actually emitted.

## Notes

- Not a correctness issue and not covered by an accepted tradeoff: the capacity page only disclaims
  linearity. Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `sse/window.rs:396-409`: `stats()` calls `scan(0)`. `scan` (`:745-760`) runs `token_scratch` across all fragments, then `visit_bucket` copies and JSON-decodes every retained fragment (`:708-744`). No running counter exists.
- `ai/guards/semantic/response.rs` calls `self.window.stats()` per retained record (`:500,588,660`), so the cost per window is quadratic in record count.
- `window-capacity-admission.md` disclaims amortized linearity but does not accept the cost.
- The side note is also accurate: `output_buffer()` (`response.rs:125-130`) allocates `MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES` per callback. The CPU figures are estimates, not measurements.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-gateway/src/sse/` is unchanged between feature-08-30 and HEAD: `RawWindow::stats` (`window.rs:396`) still calls `scan(0)` (`:745`), which re-decodes every retained fragment via `token_scratch`/`visit_bucket` (`:708-744`).
- `ai/guards/semantic/response.rs` diff touches only the empty-snapshot check, the decision-response cap and `emit_guardrail_rejection` arguments; `process_batch` (`:450`) still calls `self.window.stats()` per record (`:500`, `:588`, `:660`) and `output_buffer()` (`:125`) is unchanged.
