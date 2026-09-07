---
name: gateway-terminated-stream-settles-its-reservation
description: Use when reviewing AI attempt settlement or attempt-ledger findings; covers why a gateway-initiated mid-stream cut reconciles its credential reservation instead of retaining it, and why it carries no `incomplete_response` failure.
status: fixed
finding_id: D2 (also closes D4)
closed: 2026-08-05
knowledge_type: regression-guards
---

# A gateway-terminated AI stream settles its credential reservation and is not an upstream truncation — FIXED

## Conclusion

Terminating a streaming AI response stops it immediately, so **no further task —
including the `end_of_stream = true` one — ever reaches the response-body
hook**. That is by design (response-body pipeline spec §3.4: settlement must not
depend on the eos callback). `AiAttemptRuntimeState::finalize` nevertheless read
"no eos" as "the upstream truncated", which produced two defects with one root
cause:

- **D2** — the attempt kept `SettlementOperation::Retain`, so the credential's
  whole `estimated input + maxOutputTokens` TPM reservation stayed charged for
  the rest of the 60-second window. A handful of over-quota users could exhaust
  a shared credential's budget and cause `DeniedTpm` → `AiAttemptAdmission::Skip`
  **for unrelated tenants on the same credential**.
- **D4** — every terminated stream stamped
  `genai.attempts[].failure = "incomplete_response"` against a healthy provider,
  contradicting the `AiQuotaOverLimitTerminated` status on the very same access
  log line.

Fixed on 2026-08-05: `AiUsageShared` carries a `gateway_terminated` latch set in
`terminate_ai_stream` (`ai/usage.rs`) — the single place that decides a
deliberate cut — and settlement treats `eos || gateway_terminated` as "the
exchange is over and its consumption is knowable".

Re-reports of the form "resurrect the eos callback after a terminate", "a
terminated attempt should keep its reservation because the outcome is unknown",
or "`incomplete_response` is correct because the response really was incomplete"
are **not accepted**.

## Core Rationale

**1. "No eos" is two different facts, and only one of them is unknown**

An absent eos means either the upstream truncated (consumption genuinely
unknown → retaining the conservative charge is what protects the credential) or
the gateway ended it (the gateway is dropping that upstream connection here and
now — H1 close, H2 `RST_STREAM CANCEL` — so consumption is knowable). Retaining
on the second case protects nothing; it only holds the unconsumed remainder
against every other tenant sharing the credential. The latch restores the
distinction that `finalize` needs, at the only site that has it.

**2. The latch belongs on the terminate path, not on the eos path**

Delivering an eos after a terminate is explicitly rejected: the pump cannot do
it without reintroducing the post-terminate byte-leak class this feature already
had to fix twice, and the spec forbids depending on it. Setting a second latch
in `terminate_ai_stream` costs one store on a path that runs at most once per
request and requires no new plumbing — `AiUsageShared` is already the same `Arc`
shared by `UsageExtractionProcessor`, `AnthropicResponseProcessor`, and
`AiAttemptRuntime`.

**3. It covers both terminations, because both are deliberate**

`terminate_over_quota` (limit reached) and `terminate_quota_unenforceable`
(SSE degraded, quota no longer evaluable — see
[sse-degradation-must-not-disable-enforcement.md](sse-degradation-must-not-disable-enforcement.md))
share the helper. Both are gateway-initiated policy actions with the identical
leak and identical false `incomplete_response`; latching in the shared helper is
also strictly smaller than latching in one caller.

**4. The outlier-detection verdict deliberately still requires a real eos**

`record_success_once()` stays gated on `eos` alone. A gateway-initiated cut says
nothing about the credential's health, so it must not be committed as a success
sample. This is verdict-neutral for ejection: `HalfOpenProbe::drop` only calls
`release_probe`, so a tenant cannot use quota terminations to eject a shared
provider for everyone else. TTFT/adaptive sampling (`successful_final`) is
unchanged for the same reason.

**5. Accepted accounting bias, stated because this is billing-adjacent**

On the Anthropic path the reconciled estimate can be short by roughly one
chunk's characters ÷ 4: `AiAttemptResponseProcessor` breaks its inner chain
after a processor terminates (spec §3.4), so `UsageExtractionProcessor` — the
only writer of `output_chars` — never observes the terminating chunk's
translated output. That is a bounded, systematic tilt toward **under**-charging
(tens of tokens against a reservation of thousands), accepted rather than
corrected because the correction would mean running processors after a
terminate. The comment on `settlement_for` states the direction explicitly.

## Fix Suggestions Not Accepted

- **"Make the pump deliver eos after a terminate"** — forbidden by the design
  spec and reintroduces the post-terminate byte-leak class.
- **"Retain is the safe default; leave it"** — retention is safe only against
  *unknown* consumption. Against a known cut it is a cross-tenant capacity leak
  with a 60-second blast radius per request, not a conservative choice.
- **"Also mark the attempt successful for outlier detection"** — a policy cut is
  not evidence the credential is healthy; see rationale 4.
- **"Correct the Anthropic one-chunk under-count by observing before breaking"**
  — see rationale 5; it trades a few tokens for the failure class the terminate
  ordering exists to prevent.
- **"Drop `incomplete_response` entirely / reclassify it"** — it remains the
  correct label for a genuine provider truncation; only the gateway-initiated
  case stops claiming it.

## Re-evaluation Triggers

Re-open this decision only if:

- A third deliberate-termination site appears that does not route through
  `terminate_ai_stream` — it would silently reacquire the leak.
- The response-body pipeline gains a settlement callback that survives a
  terminate, which would make the latch redundant.
- The Anthropic path gains its own `output_chars` writer, which would remove the
  accepted under-count in rationale 5.

## Reference Cases

- D2 / D4, 2026-08-05 — the original finding, from the response-body pipeline
  correctness audit (task file closed and removed with this fix)
- `edgion-gateway/src/ai/usage.rs` — `terminate_ai_stream`, the
  `gateway_terminated` latch on `AiUsageShared`
- `edgion-gateway/src/ai/attempt_runtime.rs` — `AiAttemptRuntimeState::finalize`
  and `settlement_for`
- Regression tests:
  `ai::attempt_runtime::tests::a_gateway_terminated_stream_reconciles_its_reservation_and_is_not_an_incomplete_response`
  (drives the real `on_chunk` coupling instead of constructing the latches),
  `ai::usage::tests::quota_over_limit_appends_an_error_event_and_terminates_without_dropping_the_current_chunk`,
  and the integration case
  `quota_terminated_stream_records_its_status_and_settles_its_bucket` in
  `edgion-tests/src/client/suites/edgion_backend/single_provider.rs`
- `docs/en/ops-guide/observability/access-log.md` — what `failure` now means

## Resolution references

- `D2 (closes D4)` — 2026-08-05.
