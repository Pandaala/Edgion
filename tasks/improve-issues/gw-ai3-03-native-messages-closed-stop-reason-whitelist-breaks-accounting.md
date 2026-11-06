# gw-ai3-03: Native Messages usage observation validates `stop_reason` against a closed list — a new Anthropic stop reason kills every quota-Reject stream and drops accounting fleet-wide

- **Category**: design-error
- **Severity**: P2 (downgraded from P1 by independent verification; latent, externally triggered, fleet-wide)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/protocols/anthropic/usage.rs:433-447` (`Key::Stop` → `stop` only for 7 known values, otherwise `stop_invalid = true`)
  - `edgion-gateway/src/ai/protocols/anthropic/usage.rs:107-126` (`Observer::finish`: `invalid |= s.stop_invalid`)
  - `edgion-gateway/src/ai/protocols/anthropic/processor.rs:81-131` (`event`: `observed.invalid` → `mark_observation_unavailable`; `Kind::Stop` without prior `stop_metadata` → unavailable)
  - `edgion-gateway/src/ai/protocols/anthropic/processor.rs:280-303` (quota+Reject+streaming: unavailable observation → `QUOTA_UNAVAILABLE_SSE_EVENT` + terminate)
  - `edgion-gateway/src/ai/protocols/anthropic/processor.rs:146-161` (non-streaming: `!observed.stop` → `mark_observation_unavailable`)
  - Settlement: `ai/runtime/attempt_runtime.rs:534-564` (unknown usage → `Retain` full reservation), `ai/accounting/finalize.rs:113-160` (business quota `unresolved`)

## Summary

On the native `/v1/messages` passthrough path Edgion does not translate `stop_reason`; it only
needs usage numbers. Yet the usage observer treats any `stop_reason` outside a hard-coded list
(`end_turn`, `max_tokens`, `stop_sequence`, `tool_use`, `pause_turn`, `refusal`,
`model_context_window_exceeded`) as an invalid document. Anthropic has added stop reasons several
times (`pause_turn`, `refusal`, `model_context_window_exceeded` were all added during 2025). The day
the provider ships the next one, every native Messages response that uses it is classified
"observation unavailable", which (a) terminates streaming responses on routes with a Reject quota
right at `message_delta`, replacing `message_stop` with a gateway error event, and (b) retains the
full `input + max_tokens` credential reservation and leaves the business quota `unresolved` for all
other routes — until Edgion ships a code change.

## Current behavior (evidence)

1. `message_delta` carries `delta.stop_reason`. In the selector, `on_captured_string` for `Key::Stop`
   sets `self.stop = matches!(s, "end_turn" | ... | "model_context_window_exceeded")` and otherwise
   `self.stop_invalid = true` (`usage.rs:433-447`).
2. `Observer::finish` folds `stop_invalid` into `Observation.invalid` (`usage.rs:120-123`).
3. `MessagesProcessor::event` → `observed.invalid` → `shared.mark_observation_unavailable()`
   (`processor.rs:91-93`); the following `message_stop` also fails `stop_metadata`
   (`processor.rs:113-117`).
4. With quota configured, `requested_streaming` and `onExceeded: Reject`
   (`changes_body_length()`), the same callback terminates the stream with
   `QUOTA_UNAVAILABLE_SSE_EVENT` (`processor.rs:283-303`). Anthropic SDKs raise on `event: error`,
   so the client sees a failed request after receiving the whole answer.
5. Without Reject (or non-streaming, `processor.rs:146-161`), usage becomes `Unknown`:
   `settlement_for` returns `SettlementOperation::Retain`, holding the request's whole credential
   reservation for the Redis window, and `finalize_ai_quota` calls `finalize_unknown()`.

## Industry comparison

- Provider SDKs (anthropic-python/TS) model `stop_reason` as an open string and pass unknown values
  through. Envoy AI Gateway and LiteLLM extract `usage.input_tokens/output_tokens` without validating
  the stop reason for accounting purposes.
- Edgion itself treats unknown SSE *event names* as transparent on this path (`processor.rs:219-222`),
  i.e. it already chose forward compatibility for the envelope but not for this scalar.

## Impact

A routine provider API addition becomes a production incident for every Edgion deployment using
native Messages with quotas: streams cut at the end, retained TPM reservations starving shared
credentials (the same starvation mechanism described in gw-ai-01), and unresolved tenant quotas —
with no operator-side mitigation short of disabling quota or observation.

## Suggested direction

- In the native observer, accept any non-empty bounded string as a terminal `stop_reason` for
  *accounting* purposes (keep the 64-byte capture bound and the null/wrong-type checks). Lifecycle
  validity should depend on structure (start → delta with usage → stop), not on the value set.
- Keep the closed mapping only where meaning is required: the Chat↔Anthropic translator
  (`chat_to_anthropic/response.rs:1267`), per the existing knowledge page.
- Optionally record an `EdgionErrorCode`/metric for "unrecognized stop_reason" so operators notice
  provider changes without losing accounting.

## Notes

- `skills/04-review/ai-routing/regression-guards/anthropic-pause-turn-is-a-successful-stop.md` says
  "unknown values still fail closed so provider wire changes cannot silently acquire incorrect
  meaning". That rationale applies to the *translators*, where the value is mapped to an OpenAI
  `finish_reason`. On native passthrough the value is forwarded byte-for-byte and never interpreted,
  so there is no meaning to acquire; failing closed only destroys accounting and, under Reject,
  the response. This is new evidence that the closed-list policy is over-applied, not a
  re-litigation of the translator decision.
- `native-messages-preservation-and-accounting.md` (line ~62) lists stop_reason among permitted
  delta patches but does not address unknown values on the native path.

## Verification (independent)

- **Verdict**: DOWNGRADED — P1 → P2.
- Mechanism confirmed: `ai/protocols/anthropic/usage.rs:433-447` sets `stop_invalid` for any value outside the 7 known reasons. `Observer::finish` folds it into `invalid` (`usage.rs:120-124`). `processor.rs:91-93` marks observation unavailable. `processor.rs:283-303` terminates Reject-quota streams with `QUOTA_UNAVAILABLE_SSE_EVENT`.
- The fail-closed rationale in `anthropic-pause-turn-is-a-successful-stop.md` is explicitly about translator mapping. The native passthrough never interprets the value, so the finding's scope argument holds.
- Reason for the downgrade: no current provider value triggers it. This is a latent forward-compatibility hazard with a small fix, not an active defect. It becomes an incident only when Anthropic ships a new stop reason (precedent: three were added in 2025).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `ai/protocols/anthropic/usage.rs` and `processor.rs` are unchanged between feature-08-30 and HEAD (the only anthropic diff is a removed `Default` derive in `request.rs`).
- `usage.rs:433-447` still sets `stop_invalid` for any value outside the 7 known reasons; `usage.rs:122` folds it into `invalid`; `processor.rs:92` marks observation unavailable and `processor.rs:280-303` still emits `QUOTA_UNAVAILABLE_SSE_EVENT` on Reject-quota streams.
- No new review page extends the closed-list rationale to native passthrough.
