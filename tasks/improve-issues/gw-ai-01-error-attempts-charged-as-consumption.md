# gw-ai-01: Definitive provider error responses are charged as token consumption (or retain full credential reservations)

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/runtime/attempt_runtime.rs:177-191` (`charged_tokens` / `final_charged_tokens`, no status gate)
  - `edgion-gateway/src/ai/runtime/attempt_runtime.rs:401-460` (`finish_failure` -> `Reconcile(estimated)` / `Retain`)
  - `edgion-gateway/src/ai/runtime/attempt_runtime.rs:534-564` (`settlement_for`, status only gates the outlier verdict)
  - `edgion-gateway/src/ai/runtime/attempt_runtime.rs:570-620` (`commit_ledger` adds every attempt to `charged_request_tokens` and LowestUsage telemetry)
  - `edgion-gateway/src/ai/accounting/usage.rs:502-560` (`accounting_snapshot`, estimation fallback), `:934-990` (`finish`, `JsonUsageOutcome::Missing` => complete)
  - `edgion-gateway/src/ai/accounting/finalize.rs:113-160` (`finalize_ai_quota` uses `charged_request_tokens`)
  - `edgion-gateway/src/ai/runtime/dispatch.rs:549-553, 641-647` (reported-only selection; reservation = estimated input + request/default max output)
  - `edgion-gateway/src/ai/http/attempt.rs:400-467` (`on_raw_response`, status retry path)

## Summary

AI accounting never asks whether the provider actually performed billable work. Every dispatched
attempt — including a provider `429`/`503` that is retried, and a final non-2xx such as `400
context_length_exceeded` or `401` — is settled exactly like a successful one. On the OpenAI Chat
path (estimation allowed) such attempts charge the estimated prompt (`ceil(chars/4)`, plus
`ceil(error_json_chars/4)` for a final error body) to the tenant's business `tokenQuota`, the
estimated USD cost, credential TPM reconciliation, and the `LowestUsage` telemetry. On the
reported-only paths (native Responses / Messages) the same error responses are classified as
"unknown usage", which retains the full `input + max_output` credential reservation until the
Redis window expires. Both outcomes are wrong for a definitive, unbilled provider rejection.

## Current behavior (evidence)

1. Retried status (e.g. provider 429 with route `retry.codes` containing 429):
   `on_raw_response` (`http/attempt.rs:409`) returns the retry error before any body is processed, then
   `finish_failure(AiAttemptFailure::Known(..))` (`attempt_runtime.rs:410-430`) calls
   `active.charged_tokens()`. `accounting_snapshot(estimated_input, false)` (`usage.rs:502`) finds no
   reported usage, JSON not started, observation not off → returns
   `Estimated { input_tokens: estimated_input_tokens, output_tokens: 0 }`. That value goes to
   `SettlementOperation::Reconcile(charged)` for the credential and to `commit_ledger`, which adds it
   to `charged_request_tokens` (`attempt_runtime.rs:587-593`) and `adaptive.record_charged_tokens`
   (`:615-618`).
2. Final non-2xx (no retry): the JSON error body is fed through `observe_non_streaming`; at EOS
   `JsonUsageObserver::finish` returns `Missing` for a valid `{"error":{...}}` document, which
   `UsageExtractionProcessor::finish` treats as `json_complete` (`usage.rs:978-980`). The snapshot is
   then `Estimated { input = prompt/4, output = error_json_chars/4 }`. `settlement_for`
   (`attempt_runtime.rs:535`) only uses `status` to decide whether to record an outlier *success*; the
   reconcile amount ignores it. `finalize_ai_quota` (`finalize.rs:124`) finalizes the business quota
   with `charged_request_tokens`, and `finalize_ai_usage` publishes an `estimated_cost_usd`.
3. Reported-only APIs (`dispatch.rs:549`: everything except OpenAI Chat): an error body has no
   usage, so `accounting_snapshot` returns `Unknown / MissingProviderUsage` (`usage.rs:523-528`);
   `settlement_for` → `final_charged_tokens() == None` → `SettlementOperation::Retain`, and
   `finish_failure(Known)` also falls to `Retain`. The reservation reserved at admission was
   `estimated_input + (request max_tokens or backend default)` (`dispatch.rs:643-647`), so one
   provider-rejected request holds that whole amount against the shared credential's TPM until the
   minute window (+ grace) expires. The business quota finalizes `unresolved`.

Consequences in concrete numbers: a 120k-token prompt to a route with `retries: 3` during a provider
429/5xx storm charges ~480k tokens to the caller's business quota and logs a cost for four attempts
although the provider billed nothing. A native-Messages client that sends requests the provider
rejects with 400 (e.g. `max_tokens` above the model limit but below the credential TPM) pins a
large reservation on the shared credential for ~60 s per request — a cheap way for one tenant to
starve every tenant sharing that credential pool.

## Industry comparison

- Envoy AI Gateway extracts token usage from the response body and feeds `hits_addend` to the rate
  limit service only from the parsed usage; an error response has no usage and costs 0.
- Kong `ai-rate-limiting-advanced` / `ai-proxy` count tokens from the provider's `usage` of
  successful responses; error responses are not converted into prompt-token charges.
- LiteLLM budget/spend tracking runs in the success callback; failure callbacks log the error with
  zero spend, and its router "cooldown" handles 429s without charging the user budget.
- All of them treat a definitive provider 4xx/5xx as "no consumption", and only ambiguous transport
  outcomes (timeouts/resets after send) as "unknown".

## Impact

- Tenants exhaust business budgets and see inflated cost during provider incidents, exactly when
  retries/failover are active — the opposite of what failover is meant to provide.
- Access-log cost (the documented durable billing source) over-reports.
- Credential TPM capacity is held by rejected requests on reported-only APIs (cross-tenant DoS
  amplification); `LowestUsage` routes away from a backend because of its own failures counted as
  usage (mixing health with load).

## Suggested direction

Add a single settlement classification that runs before usage projection:

1. **Definitive non-success** (final HTTP status outside 2xx, or a status retry decided in
   `on_raw_response`): if the provider reported usage, charge it; otherwise charge **zero** for
   business quota, cost and adaptive usage, and `Release`/`Reconcile(0)` the credential reservation.
   Keep the per-attempt ledger row (status, failure) for observability.
2. **Ambiguous** (timeout/reset after send, truncated 2xx body, unparsable 2xx JSON): keep the
   current conservative estimate/retain behavior.
3. **2xx complete**: current behavior.

Implementation sketch: carry the attempt's final status into `accounting_snapshot` (or make
`charged_tokens`/`final_charged_tokens` return `Some(0)` when `status` is non-2xx and no reported
usage exists). Optionally keep a configurable "charge prompt on provider 4xx" knob only if a provider
is known to bill such errors — but default must be zero.

## Notes

- Checked `skills/04-review/ai-quota/*` (all regression guards concern SSE degradation, gateway
  cut, retention) and `ai-routing/*`; none classifies error responses. Docs
  (`docs/en/ai-gateway/quotas-and-cost.md`: "charges accountable input and output across all
  dispatched attempts, including retries"; `credentials-and-limits.md#reservation-semantics`:
  "unknown usage retains the reservation") describe retries and unknown usage, but neither states
  that a definitive provider error is billed or treated as unknown; the code simply lacks the
  status distinction. "Accountable" should exclude unbilled rejections.
- `gateway-terminated-stream-settles-its-reservation.md` already establishes that "settled with
  known outcome" must not be treated as ambiguous — this finding extends the same reasoning to
  provider error statuses.
- Related but distinct: `tasks/todo/ai-todo/request-output-ceilings.md` (bounding client
  `max_tokens`) would reduce, not remove, the reported-only retention problem.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `attempt_runtime.rs:402-430` (`finish_failure`, Known) reconciles with `charged_tokens()` and commits the ledger; there is no status check. `settlement_for` (`:535-565`) uses `status` only to gate `record_success_once`; the reconcile amount comes from `final_charged_tokens()`.
- `usage.rs:502-560` `accounting_snapshot`: with no reported usage and a non-reported-only API it returns `Estimated{input=estimated_input}`; on reported-only APIs it returns `Unknown/MissingProviderUsage`, which becomes `SettlementOperation::Retain`.
- `usage.rs:978-980`: `JsonUsageOutcome::Missing` (a valid error JSON document) marks `json_complete`, so a final 4xx body is charged as an estimate.
- `commit_ledger` (`:571-620`) adds every attempt to `charged_request_tokens` and `adaptive.record_charged_tokens`. No `skills/04-review/ai-quota` or `ai-routing` page classifies provider error statuses, and `docs/en/ai-gateway/quotas-and-cost.md:19` / `credentials-and-limits.md:43` do not say unbilled errors are charged.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `git diff feature-08-30..HEAD` on the cited files shows only cosmetic changes (`metric_totals` no longer returns `Result`, `fetch_update`→`try_update`, merged `Skip|DependencyDenied` arm in `select_peer`); no status-aware settlement was added.
- `attempt_runtime.rs:401-433` `finish_failure(Known)` still reconciles `charged_tokens()` (or `Retain`) with no status check; `settlement_for` (`:534-564`) still uses `status` only for `record_success_once`, and `commit_ledger` (`:570-620`) still adds to `charged_request_tokens` and `record_charged_tokens` (`:617`).
- `usage.rs:979` `JsonUsageOutcome::Missing` still completes the JSON as estimable; `dispatch.rs:549-553` still selects reported-only for non-Chat APIs; `finalize.rs:127` still finalizes with `charged_request_tokens`.
