---
name: provider-rejection-settles-as-zero-consumption
description: Use when reviewing AI attempt settlement, business quota, cost, credential TPM reconciliation or LowestUsage telemetry for provider error statuses; covers why a definitive non-2xx attempt charges only reported usage (otherwise zero) instead of an estimate or a retained reservation.
status: fixed
finding_id: gw-ai-01
closed: 2026-11-06
knowledge_type: regression-guards
---

# A definitive provider rejection settles as zero consumption — FIXED

## Conclusion and scope

An AI attempt the provider answered with a non-2xx status — a status retry decided in
`on_raw_response` (`ai/http/attempt.rs`) or a final 4xx/5xx — performed no billable work
unless the provider itself reported usage. Such an attempt therefore:

- charges the provider-reported usage when present, and otherwise **zero** input and output
  tokens to the business `tokenQuota`, attempt ledger, access-log summary, cost, token metrics
  and `LowestUsage` adaptive telemetry;
- reconciles the credential reservation to that amount (`Reconcile(0)` keeps RPM=1 and returns
  the TPM reservation), including when the error body is not JSON (an HTML gateway page) or is
  cut before EOS;
- keeps its ledger row (`status`, `failure`) for observability, with
  `usage_source: estimated` and zero tokens.

Intermediary timeout statuses 504 and 524 are not definitive: a proxy in front of the
provider may time out while the provider still generates, so they keep the ambiguous
behavior (estimate or retain) like a transport timeout.

Scope limits: `usage: Off` (not observed) is unchanged and still retains. 2xx paths are
unchanged, including reported-only `Unknown` retention and native `response.failed` events
on a 2xx stream. Ambiguous transport failures (`Ambiguous`, `BackendRequestTimeout`) still
retain the credential reservation; if a non-2xx header preceded them, their ledger usage is zero.

## Rationale and evidence

Before the fix, settlement never consulted the HTTP status: `AiUsageShared::accounting_snapshot`
returned `Estimated{prompt/4 [+ error JSON chars/4]}` on OpenAI Chat (a valid
`{"error":...}` document completes as `JsonUsageOutcome::Missing`), and
`Unknown/MissingProviderUsage` on reported-only APIs, which became `Retain` of the whole
`input + maxOutputTokens` reservation for the 60 s window. A 120k-token prompt with
`retries: 3` during a provider 429 storm charged ~480k tokens; a native Messages tenant sending
provider-rejected requests could pin shared credential TPM against every other tenant.

Root cause: the status was recorded only on `AiAttemptRuntime` and used only for the outlier
verdict. The fix records it on the per-attempt `AiUsageShared`
(`observe_upstream_status`, called from `AiAttemptRuntime::record_response`), and
`accounting_snapshot` — the single source for credential settlement, ledger, request summary
(`ai/accounting/finalize.rs`) and quota — returns zero usage for `provider_rejected()` without
reported usage. `settlement_for` lets a provider-rejected attempt bypass the transport-ambiguity
`Retain`. Gating only `charged_tokens` in the runtime would have left the access-log summary
cost inflated.

This extends [gateway-terminated-stream-settles-its-reservation](gateway-terminated-stream-settles-its-reservation.md):
retention protects only against unknown consumption; a definitive provider outcome is known.

## Alternatives and limits

- **"Charge the prompt estimate on 4xx because some provider might bill it"** — rejected as the
  default; mainstream providers do not bill rejected requests. A provider that does bill must
  report usage in the error response, which is still charged.
- **"Add a new `usage_source` label for rejections"** — not done; the access-log schema stays
  unchanged and `status` already identifies the rejection.
- Cost: a provider that silently bills an error without reporting usage is under-charged.
  The "providers do not bill rejected requests" premise is an assertion, not a sourced
  billing contract; the industry comparison in the finding covers gateways only.
- A non-JSON 502 page from an intermediary is treated as definitive; only 504/524
  timeouts are excluded. Rejected attempts on reported-only APIs now log `estimated`
  with zero tokens (documented in `docs/en/ai-gateway/observability.md`).
- Asymmetry kept: a transport failure (`Ambiguous`, `BackendRequestTimeout`) after a
  non-2xx header logs zero usage but still retains the credential reservation.

## Verification and re-evaluation

- Unit test `ai::runtime::attempt_runtime::tests::definitive_provider_rejections_charge_only_reported_usage`
  covers retried 429, final 400 with an error body, reported-only APIs, a non-JSON 502 with and
  without EOS, reported usage on an error, 504/524 staying ambiguous, and the unchanged
  `Off` retention. Integration expectations in `edgion-tests` (`priority_retry_business_quota`,
  `priority_retry_attempt_ledger`, credential exhaustion TPM) were updated but not run here.

Re-evaluate if intermediary statuses other than 504/524 (for example 502) are shown to
hide continued provider generation, if a supported provider is shown to bill error responses without reporting usage,
if `record_response` stops being the single status entry for AI attempts, or if a new settlement
path computes usage without `accounting_snapshot`.

## Resolution references

- `gw-ai-01` (improve-issues, CONFIRMED P1) — fixed 2026-11-06.
