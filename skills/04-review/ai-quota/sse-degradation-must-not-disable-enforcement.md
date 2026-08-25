---
name: sse-degradation-must-not-disable-enforcement
description: Use when reviewing AI streaming quota findings; covers what happens when the SSE decoder degrades mid-stream and why enforcement fails closed rather than silently switching off.
status: fixed
finding_id: D1
closed: 2026-08-05
---

# SSE decoder degradation must not silently disable mid-stream quota enforcement — FIXED

## Conclusion

`SseDecoder::degrade()` sets a permanent latch, after which `feed_each` returns
immediately for the rest of the stream. Because `quota.add_output_units` is
only ever called from inside `feed_each`'s visitor closure, degradation used to
switch mid-stream quota enforcement off silently and without fallback: the
accumulator stopped growing, the quota worker stopped flushing (`run_worker`
skips a flush when `target == charged_total`), `over_limit()` could never flip,
and the upstream billed freely until the request timeout.

Fixed on 2026-08-05 in two parts: the incomplete-event cap was raised from
64 KiB to **1 MiB**, and degradation now **fails closed** when quota enforcement
is active.

Re-reports of the form "degradation stops token counting", "one big SSE event
bypasses the quota", or "the character counter keeps working so this is fine"
are **not accepted** — verify the degradation arm in
`UsageExtractionProcessor::on_chunk` before re-filing.

## Core Rationale

**1. The decoder was never the defect**

The degrade latch is correct as memory safety: an upstream that never emits an
event boundary would otherwise grow the buffer without bound. The defect was
that `ai/usage.rs` made quota enforcement implicitly depend on parse success —
a memory-safety switch had been wired to a cost-control guarantee with no
fallback and no operator signal.

Note the asymmetry that hid it: `add_output_chars` is called **before**
`feed_each` and kept working after degradation, so the existing test
`sse_overflow_degrades_without_stopping_character_count` passed. But the two
counters feed different things — chars feed post-hoc estimation (which has a
`chars/4` fallback in `final_charged_tokens`), units feed the enforcement latch
(which had none).

**2. The Anthropic path had already chosen fail-close**

`AnthropicResponseProcessor::process_streaming` (`ai/anthropic/response.rs`)
treats `decoder.degraded() || decoder.malformed()` as a translation failure and
seals the stream. Only the OpenAI-schema path was exposed, because
`ai/dispatch.rs` gives the Anthropic provider's quota accumulator to that
processor and passes `None` to `UsageExtractionProcessor`. Making the OpenAI
path fail closed too removed an inconsistency rather than inventing a policy.

**3. Raising the cap is what makes fail-close defensible**

At 64 KiB (≈16K output tokens) a "fake streaming" provider — one that emits the
whole completion as a single event, common among OpenAI-compatible aggregators
and self-hosted shims — trips degradation with a perfectly legitimate payload.
Failing closed there would cut real users' streams. At 1 MiB (≈256K tokens) no
shipping model's one-shot output reaches the cap, so the residual population is
genuinely pathological and "a stream we cannot parse is a stream we cannot
bound" holds. 1 MiB also matches `ai::usage::MAX_NON_STREAMING_USAGE_BYTES`,
which already accepts the same payload in non-streaming form, and sits at the
same order as Envoy's 1 MiB default `per_connection_buffer_limit_bytes`.

**4. No new failure shape**

The degradation arm is gated on exactly the three conditions that gate the
existing over-limit arm (`quota.is_some() && requested_streaming &&
terminate_on_over_limit`), so `changes_body_length()` continues to describe both
terminations, `Throttle`/`Downgrade` still mean "do not hard-fail this user",
and an unquota'd request is never cut for a framing reason. The injected event
reuses `AiProxyError::QuotaUnavailable`, the same error the admission path
already renders when enforcement cannot be performed.

**5. The injected event needs its own SSE boundary here, unlike the over-limit arm**

Degradation can only happen *while an event is unterminated* — that is the
condition that trips the cap — so this injection always lands mid `data:` line.
`QUOTA_UNENFORCEABLE_SSE_EVENT` therefore carries an unconditional leading
`\n\n`; without it the diagnostic would be concatenated onto the partial line
and read as one malformed event, i.e. guaranteed unreadable exactly where it is
needed. This fix left the over-limit arm alone (landing mid-event is possible
but not certain there); it was closed separately as D3 —
[injected-error-event-needs-its-own-sse-boundary.md](injected-error-event-needs-its-own-sse-boundary.md)
— and both constants now share one boundary-emitting builder.

**6. What raising the cap costs**

Worst-case resident memory per decoder grew 16x. The cap bounds accounted bytes,
but `pending` + `cur_data` carry amortized-doubling slack (budget ~1-2 MiB), and
an Anthropic request runs **two** decoders (translator + usage, see
`ai::dispatch`), so that path doubles again. There is no aggregate bound across
concurrent streams. Accepted because reaching it requires a hostile or broken
**provider** — an operator-configured upstream, not an arbitrary client.

## Fix Suggestions Not Accepted

- **"Reset `degraded` at the next event boundary"** — the decoder cannot find a
  boundary it has already overrun, and a resettable latch reintroduces the
  unbounded-growth hazard the cap exists to prevent.
- **"Derive output units from `sse_chars` after degradation (Option A)"** —
  considered and rejected: it keeps proxying a stream whose framing is not
  understood while charging by estimate, and it diverges from the Anthropic
  path's existing fail-close. Reconsider only if cutting pathological streams
  turns out to hurt real traffic.
- **"Make the cap configurable"** — deliberately out of scope for the fix; a CRD
  field plus validation plus paired docs is a feature, not a review fix. The
  constant is sized against real model output bounds instead.
- **"Emit a metric for degradation"** — the project routes data-plane signals
  through the access log and `EdgionErrorCode`, not new metrics. Two codes were
  added instead: `AiSseDecodeDegraded` (precision loss, any request) and
  `AiSseDegradedQuotaTerminated` (a stream was cut).
- **"Terminate on degradation regardless of quota"** — degradation predates
  mid-stream enforcement and is harmless on its own; an unquota'd stream must
  keep running and fall back to the character estimate.

## Scope boundary — malformed-event sibling

The rejections above are about the **degradation** path only. The sibling
malformed-event bypass was fixed separately as D1b:

- **`malformed()` — D1b, fixed 2026-08-11.** The decoder counts completed
  poisoned events cumulatively. OpenAI quota observation tolerates one and
  fails closed on the second under the existing enforcement gate. See
  [sse-malformed-events-must-not-disable-enforcement.md](sse-malformed-events-must-not-disable-enforcement.md).
- **The injected event's SSE boundary on the over-limit arm** — closed
  separately as D3, see
  [injected-error-event-needs-its-own-sse-boundary.md](injected-error-event-needs-its-own-sse-boundary.md).

## Re-evaluation Triggers

Re-open this decision only if:

- A model ships whose single-event output can legitimately exceed 1 MiB, making
  the cap itself the source of false positives again.
- Real traffic shows `AiSseDegradedQuotaTerminated` firing on legitimate
  providers, which would argue for the Option A fallback after all.
- The SSE decoder is rebuilt as the zero-copy byte-stream abstraction tracked in
  `tasks/todo/sse-decoder-zero-copy-stream.md` — the requirement carries over,
  but the implementation site moves.

## Reference Cases

- `tasks/todo/d1-sse-degradation-disables-quota-enforcement.md` — the original
  finding, from the response-body pipeline test-coverage audit
- `edgion-gateway/src/ai/usage.rs` — `terminate_quota_unenforceable`,
  `QUOTA_UNENFORCEABLE_SSE_EVENT`, and the degradation arm in `on_chunk`
- `edgion-gateway/src/sse/mod.rs` — `DEFAULT_MAX_PENDING_EVENT_BYTES` and its
  sizing rationale
- Regression tests: `ai::usage::tests::oversized_sse_event_must_not_disable_mid_stream_quota_enforcement`,
  `sse_degradation_without_quota_enforcement_only_records_the_signal`,
  `an_already_over_limit_stream_reports_over_limit_not_degradation`, and the
  integration case of the same name in
  `edgion-tests/src/client/suites/edgion_backend/single_provider.rs`
- `docs/en/ai-gateway/quotas-and-cost.md` — operator-facing behavior and alerts
