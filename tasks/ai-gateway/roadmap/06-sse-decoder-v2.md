# WS-06: SSE Decoder v2 — Zero-Copy Slice Chain + Hold-and-Release Windows

**Status:** ready for kickoff under `tasks/pending/sse-decoder-v2/`. ExtProc
attached response streaming is accepted. This workstream's hold-and-release
window is the hard gate for Stream Processing Work Package 2.

**Goal:** Replace the byte-at-a-time `SseDecoder` with a `Bytes`-slice-retaining
incremental decoder, add a bounded hold-and-release window primitive (unblocking
WS-03 response guards), eliminate the Anthropic double-decode, and add the
canonical bounded-window clarification note to the body-invariant docs, while
preserving the already-fixed D1b malformed-event enforcement contract.

**This workstream adopts the existing design task**
`tasks/ai-gateway/todo/sse-decoder-zero-copy-stream.md` (design-discussion
stage) — that file carries the verified facts (F1-F3), constraints, and prior
art; this file locks scope and sequencing. It also resolves decoder-adjacent
open issue D5 from
`tasks/ai-gateway/todo/response-body-pipeline-hardening.md`.

**Ownership interaction:** this workstream is backend-resource-identity
independent, but it touches AI dispatch, response processor, SSE, and Anthropic
files that may overlap the active ExtProc package. Parallel execution requires
an explicit non-overlapping file handoff; otherwise run it after ExtProc
acceptance. Backend Phase 1 is already complete.

## Why (from the comparison, §6)

- Our per-event cost today: byte-wise `pending.push` + `cur_data.push_str`
  (~2 copies of data content) + `serde_json::Value` per event (dominant).
  agentgateway's codec does one side-copy into a `BytesMut` + per-event parse.
  Parity requires dropping our extra copies; leadership requires lazy JSON.
- WS-03's streaming guards need windowed text with overlap — a capability
  neither our decoder nor theirs exposes today (they built it ad hoc in
  `streaming_guardrails.rs`).
- Anthropic route currently decodes twice (translator + usage extractor)
  (`sse/mod.rs:28-30`).

## Scope

1. **Slice-chain decoder** (`edgion-gateway/src/sse/` rewrite): retain incoming
   `Bytes` chunks (refcount) in a bounded deque; represent lines/events as
   ranges over the chain; materialize (copy) only on consumer demand
   (`event.data_bytes() -> Cow`-style accessor that is zero-copy when a field is
   chunk-contiguous — the F1 fact: pingora right-sizes H1 chunk allocations, so
   retention pins ≈ retained bytes + one chunk of slack). Same bounds/degrade/
   malformed semantics and 1 MiB pending cap; same `feed_each` visitor shape so
   consumers migrate mechanically.
2. **Lazy JSON**: replace per-event `serde_json::Value` in `usage.rs` with
   targeted extraction (the fields actually read: `usage.*`, `choices[].delta`
   presence) via streaming `serde` deserializers over the slice ranges — kills
   the dominant allocation. Keep a `Value` escape hatch for translators.
3. **Hold-and-release window primitive** (`sse/window.rs`): bounded queue of
   decoded events + running text-window view (char/token-estimate sized,
   configurable overlap), `release()` returning the original `Bytes` slices
   for downstream forwarding (or a rewritten replacement). Used by WS-03;
   designed here so the decoder owns all retention accounting.
4. **Event-stream processor interface**: the Anthropic translator emits *events*
   consumed directly by the usage extractor (no re-decode of translated bytes);
   composite processor in `dispatch.rs` wires decoder → [translate] → [guard
   window] → [usage] as one event pipeline per stream.
5. **Invariant clarification (not an amendment)**: re-analysis (2026-08-09)
   established that invariant 2 already permits bounded holding — it bans
   *unbounded* state, and the shipped Anthropic translator withholds
   partial-event bytes across chunks (`ai/anthropic/response.rs:351`
   `chunk.take()` + bounded decoder pending). Add a canonical note to
   `AGENTS.md` + skills body docs stating the agreed conditions for a window:
   bounded and admission-validated, policy-declared, hold-and-release (no
   full-stream accumulation), flushed at eos, default-off. Window×terminate
   ordering (release-before-terminate for masking; discard-on-terminate for
   reject) is documented on the trait. Chunk-driven stall behavior (a held
   window waits for the next chunk or eos when the upstream stalls) is
   accepted; a fork-level max-hold-time tick is optional P3, out of this WS.
6. **D1b regression preservation**: D1b is already fixed. The replacement must
   retain a whole-response cumulative malformed-event count, tolerate the first
   poisoned event, and fail closed on the second only for streaming quota
   `Reject` enforcement. Valid events never reset the count, and one poisoned
   event with several invalid lines counts once. D5 (sealed Anthropic stream
   never terminates) is fixed here by allowing a sealed translator to still
   request terminate.

## File map

```
Rewrite: edgion-gateway/src/sse/mod.rs        (decoder core; keep public API names)
Create:  edgion-gateway/src/sse/chain.rs      (slice-chain buffer + range types)
Create:  edgion-gateway/src/sse/window.rs     (hold-and-release window)
Modify:  edgion-gateway/src/ai/usage.rs       (lazy extraction; preserve D1b gate)
Modify:  edgion-gateway/src/ai/anthropic/response.rs  (event-emitting mode; D5)
Modify:  edgion-gateway/src/ai/dispatch.rs    (event-pipeline composition)
Modify:  AGENTS.md + skills body-invariant docs (clarification)
Tests:   sse unit suite ports 1:1 + new chain-boundary property tests
         (cross-chunk field splits, CRLF splits, BOM, degrade/malformed parity);
         ai integration: quota termination + Anthropic translation regressions
         must pass unchanged (wire-format golden tests already exist)
```

## Sequencing

1. Decoder core + parity test port (behavioral no-op for consumers).
2. Lazy JSON in `usage.rs` (measurable: allocation count per event in a bench).
3. Event pipeline (translator single-decode) — D5 fix rides along.
4. Window primitive (+ invariant clarification note) — hands off to WS-03.
5. D1b regression port: preserve its threshold, gates, precedence, statuses,
   and Redis-backed integration case unchanged.

## Acceptance

- No benchmark gate (roadmap decision 5): the copy/allocation reduction is
  verified structurally in review — the decoder core must contain no
  per-payload-byte `push`/`push_str` path and no per-event `serde_json::Value`
  in `usage.rs`'s hot extraction.
- All existing SSE/AI integration scenarios green, including
  `quota_termination_h2_framing` and the Anthropic wire-contract goldens;
  new chain-boundary property tests pass.
