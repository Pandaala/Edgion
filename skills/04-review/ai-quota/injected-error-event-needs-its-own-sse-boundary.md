---
name: injected-error-event-needs-its-own-sse-boundary
description: Use when reviewing AI mid-stream quota termination findings; covers why every gateway-injected SSE error event leads with a blank line and why that prefix must not be deleted or made conditional.
status: fixed
finding_id: D3
closed: 2026-08-05
---

# A gateway-injected SSE error event must carry its own event boundary — FIXED

## Conclusion

When a streaming AI response is cut short (over quota, or quota no longer
enforceable), the gateway pushes an SSE `error` event into the response-body
sink and terminates. Those bytes are appended **after the current chunk**,
whose end is wherever the upstream's chunk boundary happened to fall — nothing
on the observational path rewrites or realigns it. Without its own leading
boundary, an injection landing mid-event *continues* that half-delivered event:
the client parses one event whose data is truncated JSON glued to the error
JSON, which every conforming SSE client drops.

Fixed on 2026-08-05: both injected constants are now built by one helper,
`injected_sse_error_event` (`edgion-gateway/src/ai/usage.rs`), which emits an
unconditional leading `\n\n`. `QUOTA_UNENFORCEABLE_SSE_EVENT` already carried
the prefix from the D1 fix; `QUOTA_EXCEEDED_SSE_EVENT` did not, and that
asymmetry was the whole remaining surface.

Re-reports of the form "the leading `\n\n` is noise / dead bytes / a typo",
"prefix only when the decoder is mid-event", or "the injected event is present
in the body so the client can see it" are **not accepted** — byte presence is
not the property that matters, dispatchability is.

## Core Rationale

**1. The failure is silent by construction, which is what makes it serious**

Token burn still stops, the stream still ends cleanly, and the framing is still
valid HTTP. The only casualty is the diagnostic — and that diagnostic is the
entire reason the injection exists. The response was already committed with
`200` before the cut, so the status cannot carry the signal; the injected event
is the client's *only* way to distinguish a deliberate quota cutoff from an
upstream hang-up. Losing it converts a well-defined gateway action into an
indistinguishable network failure, with no error anywhere the client can see.

**2. Reachable without an adversary**

A hostile provider forces it 100% of the time by writing one byte per chunk,
but ordinary TCP segmentation reaches it intermittently with no attacker at
all. There is also one **deterministic** combination on the over-limit arm:
when a stream is already `over_limit()` *and* the decoder has just degraded,
`on_chunk`'s `else if` ordering correctly selects the over-limit arm (that is
the accurate reason to report), yet degradation implies an unterminated event
— so the unprefixed injection was guaranteed to be swallowed there. The unit
test `an_already_over_limit_stream_reports_over_limit_not_degradation`
exercises exactly that shape.

**3. Unconditional is strictly better than conditional here**

`\n\n` is correct in both states: mid-event, the first newline ends the
dangling line and the second dispatches it; at an already-clean boundary, both
are blank lines and a blank line with nothing buffered dispatches nothing. Two
bytes buy correctness with no state inspection, no coupling to decoder
internals, and no second code path to keep in sync. The Anthropic translator's
`process_streaming` re-emits only complete `data: …\n\n` events, so its output
always ends on a boundary and the prefix is a pure no-op there — it needs no
carve-out and must not get one.

**4. The right assertion is on the parsed view, not the bytes**

The test that "already covered" this path asserted
`sink.peek_extra()[0] == QUOTA_EXCEEDED_SSE_EVENT`, which compares the constant
to itself and would keep passing if the prefix were deleted. The integration
side asserted raw containment, which a swallowed event also satisfies. Both
now decode the client's-eye byte stream with the same WHATWG-conformant
`SseDecoder` a client would use and assert the error arrives as its own
dispatched event with parseable JSON.

## Fix Suggestions Not Accepted

- **Prefix only when the decoder is mid-event** (`SseDecoder::is_idle()` as a
  boundary oracle) — accurate, since `UsageExtractionProcessor` consumes
  exactly the bytes it forwards, but it buys two bytes on the wire at the cost
  of coupling the injection helper to decoder state and creating a second path
  that only the rarer branch exercises.
- **Fix it in `UsageExtractionProcessor` by realigning `chunk`** — symptom-level
  and destructive: the observational path's contract is that upstream bytes are
  forwarded verbatim (its own test asserts `body == original`), and rewriting
  the chunk would change what the client receives from the provider.
- **Add the boundary in `AnthropicResponseProcessor`** — that path is already
  immune; a translator-local prefix would leave the OpenAI-shaped path (the
  actually-affected one) broken.
- **Push the boundary as a separate `sink.push`** — spends a second per-batch
  emit budget entry for two bytes, and can drop the boundary independently of
  the event it exists to protect.
- **Keep asserting byte containment / constant equality** — a swallowed event
  satisfies both; these are the assertions that let the defect ship.

## Re-evaluation Triggers

Re-open this decision only if:

- A target client is found for which a stray blank line is not a no-op
  dispatch (it would be non-conforming to the WHATWG SSE spec, but a concrete
  report beats the spec here).
- A future injection site emits a wire format that is not SSE — the prefix is
  SSE framing and would corrupt anything else, which is exactly what the
  `inject` flag on `terminate_ai_stream` already gates.
- A response processor is added that both shares the sink and rewrites chunk
  bytes, since the "boundary is unknown but bytes are verbatim" reasoning
  above would need re-deriving.

## Reference Cases

- `tasks/todo/d3-injected-sse-event-swallowed-by-partial-event.md` (adversarial
  security audit of the response-body pipeline work, 2026-08-05)
- `edgion-gateway/src/ai/usage.rs` — `injected_sse_error_event`,
  `terminate_ai_stream`, and the
  `every_injected_event_stays_parseable_after_an_unterminated_event` /
  `an_injected_event_dispatches_nothing_extra_at_a_clean_boundary` unit tests
- `edgion-tests/src/client/suites/edgion_ai_service/single_provider.rs` —
  `byte_at_a_time_sse_delivery_still_terminates` and the shared
  `check_injected_event_is_dispatchable` assertion; fixture mode
  `quota_terminate_byte_at_a_time` in `edgion-tests/src/server/test_server.rs`
- Sibling finding D1: [sse-degradation-must-not-disable-enforcement.md](sse-degradation-must-not-disable-enforcement.md)
- `docs/en/ai-gateway/quotas-and-cost.md` — "Mid-stream termination"
