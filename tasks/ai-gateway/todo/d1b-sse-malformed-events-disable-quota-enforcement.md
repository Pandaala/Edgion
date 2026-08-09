# D1b — Invalid UTF-8 in every SSE event disables mid-stream quota enforcement

| Key | Value |
|-----|-------|
| Created | 2026-08-05 |
| Severity | High — cost-control bypass, same class as D1 |
| Reachable by | A hostile upstream provider, or a merely buggy one |
| Repo | `edgion-gateway` only |
| Found by | Subagent review of the D1 fix |
| Status | todo — needs a product call, deliberately NOT fixed alongside D1 |

## The defect

D1 closed the path where the SSE decoder **degrades** (one unterminated event
past the byte cap) and silently switches quota enforcement off. An equivalent
path remains open through the decoder's *other* sticky failure signal.

`SseDecoder::consume_line` (`edgion-gateway/src/sse/mod.rs`) sets `cur_poisoned`
when a field line is not valid UTF-8, and `dispatch` then drops the whole event
without invoking the visitor:

```rust
fn dispatch(&mut self, visitor: &mut dyn FnMut(SseEvent)) {
    if self.cur_has_data && !self.cur_poisoned {
        visitor(SseEvent { .. });
    }
    self.reset_event();
}
```

`quota.add_output_units` lives only inside that visitor
(`UsageExtractionProcessor::observe_chunk`). So a provider that places one
invalid UTF-8 byte in every event's `data:` line produces the exact D1 failure
chain:

1. Every event is poisoned and dropped before the visitor runs
2. `quota.add_output_units` is never called
3. `observed_output` never grows, so `run_worker` skips every flush
   (`target == charged_total`)
4. `over_limit()` never flips
5. The stream bills freely until the request timeout

Unlike D1 this produces **no degradation**, so the new fail-close arm never
fires, and **no `AiSseDecodeDegraded`** — it is completely silent.

## Why this was not fixed with D1

`SseDecoder::malformed()` is sticky and its own doc comment says it exists so
"protocol translators can use this sticky signal to fail closed", which
`AnthropicResponseProcessor::process_streaming` already does
(`ai/anthropic/response.rs`, `decoder.degraded() || decoder.malformed()`). The
OpenAI path is the only remaining exposure.

But a naive `|| self.sse.malformed()` in the D1 arm would cut a stream on a
**single** stray byte anywhere in the response, which is a much more aggressive
trigger than "one event exceeded 1 MiB". Legitimate providers do occasionally
emit a malformed byte without the rest of the stream being unparseable, and the
existing tolerant behavior (drop that one event, keep going) was a deliberate
choice for observational consumers.

So the shape of the fix is a product call, not a one-character patch.

## Options

**A — Fail closed on `malformed()` exactly like `degraded()`.** Consistent with
the Anthropic path and with D1's own rationale. Cost: one stray byte from an
otherwise healthy provider cuts a user's stream.

**B — Fail closed only past a threshold.** Count poisoned events (or their
share of total events) and terminate once the stream is provably unmeasurable
rather than merely imperfect. Preserves tolerance for isolated corruption.
Needs a threshold nobody has data to pick yet.

**C — Keep enforcement alive instead of terminating.** Charge poisoned events a
conservative unit estimate so the accumulator keeps advancing and the ordinary
over-limit path still fires. Never cuts a stream for a framing reason; costs
precision. This is the Option A that was rejected for D1 — reconsidering it
here is legitimate because the trigger is much more likely to be benign.

## Required test

A fixture mode emitting a stream whose every `data:` line carries one invalid
UTF-8 byte, driven on `QUOTA_TERMINATE_HOST` with `stream:true` past the
2-token limit, asserting the stream does not run to `[DONE]` unmetered.

Note the English-only rule: build the invalid bytes as raw byte escapes in the
fixture (e.g. `b"\xff"`), not as text in another language.

## References

- `tasks/todo/d1-sse-degradation-disables-quota-enforcement.md` — the sibling
  finding, fixed 2026-08-05
- `skills/04-review/ai-quota/sse-degradation-must-not-disable-enforcement.md` —
  the D1 decision, whose rationale covers this case too
- `edgion-gateway/src/sse/mod.rs` — `consume_line`, `dispatch`, `malformed()`
- `edgion-gateway/src/ai/usage.rs` — `observe_chunk`, `on_chunk`
