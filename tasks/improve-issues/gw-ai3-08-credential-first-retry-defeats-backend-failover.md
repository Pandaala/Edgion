# gw-ai3-08: Credential-first retry rounds spend the retry budget on credentials of a backend that is failing as a whole, so provider failover rarely happens

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/runtime/attempt.rs:377-411` (`finish_dispatch`: `Retryable` → `Phase::PickCredential { backend: same }`)
  - `edgion-gateway/src/ai/runtime/attempt.rs:417-436` (`retry_ambiguous_dispatch`: same)
  - `edgion-gateway/src/ai/runtime/dispatch.rs:293-361` (`handle_dispatched_outcome`: status 5xx/timeouts/transport errors all become `RetryableFailure` / ambiguous retry, without a failure-domain distinction)
  - `edgion-gateway/src/ai/http/attempt.rs:400-467` (`on_raw_response`: any status in `retry.codes`)
  - `docs/en/ai-gateway/resilience.md:35-41` ("Credential-first rounds")

## Summary

After any retryable dispatched failure, the AI attempt scheduler first tries another credential of
the *same* EdgionBackend and moves to another route backend only when that backend has no eligible
credential left. Rotating keys is the right reaction to credential-scoped failures (401/403, a
per-key 429). It is useless for endpoint-scoped failures — provider 500/502/503/529, upstream
timeouts, connection resets, TLS/DNS errors — because every credential of the backend shares the
same endpoint. With the typical `retry.attempts` of 1-3 and a pool of ≥2 keys, the whole retry budget
is consumed against the failing provider and the request returns the provider error without ever
reaching the configured secondary backend.

## Current behavior (evidence)

- `finish_dispatch(RetryableFailure)` sets `phase = PickCredential { backend: attempt.target.backend }`
  when dispatch capacity remains (`attempt.rs:399-405`); `retry_ambiguous_dispatch` does the same
  (`attempt.rs:427-431`). The backend is excluded only when `select_credential` finds no eligible
  untried credential (`attempt.rs:304-307`).
- `handle_dispatched_outcome` (`dispatch.rs:333-358`) maps every allowed retry to these two calls;
  the HTTP status / transport class is used for outlier accounting, not for choosing between
  "rotate credential" and "fail over backend".
- Passive outlier ejection is per credential (`credential_outlier.rs`), so a provider outage ejects
  keys one at a time while live requests keep burning retries on the next key.

Example: route with backends `openai-primary` (order 0, 4 keys) and `azure-fallback` (order 1),
`retry.attempts: 2`, `retry.codes: [429, 500, 502, 503]`. OpenAI returns 503 for all keys: the
request is sent to key1, key2, key3 of `openai-primary` and fails; `azure-fallback` is never tried
until enough requests have individually ejected all four keys.

## Industry comparison

- Envoy: `retry_host_predicate: previous_hosts` and `retry_priority: previous_priorities` move retries
  to a different host/priority after a failure.
- LiteLLM Router: `fallbacks` are applied on 5xx/timeout, while per-deployment cooldown handles
  per-key 429.
- Portkey/Kong AI Proxy fallback strategies trigger on status classes and switch *targets*, not keys.

## Impact

The headline AI-gateway resilience feature (multi-provider failover) does not engage during the
outage class it exists for; latency grows by the number of futile key retries; and retry budgets
(`retry_budget`) are drained against a dead provider.

## Suggested direction

Classify the failure domain at the point where the outcome is recorded and pass it to the
scheduler:

- credential-scoped (401, 403, 429 with per-key semantics, provider "invalid_api_key"): rotate
  credential within the backend (current behavior);
- endpoint-scoped (5xx, 529, timeouts, connect/TLS/DNS, ambiguous transport): exclude the backend
  for this request (`tried_backends |= bit(backend)`) and go to `PickBackend`.

Optionally expose a per-route knob, but the default should follow the failure domain.

## Notes

- This is the documented design ("Credential-first rounds", `docs/en/ai-gateway/resilience.md`).
  New evidence that the tradeoff is wrong: the documented rationale only covers credential-scoped
  failures, while the same path is taken for endpoint-scoped failures where credential rotation
  cannot succeed by construction. No `skills/04-review` page accepts the endpoint-scoped case.
- Related: gw-ai-01 (each futile retry is also charged as consumption), gw-ai-02.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `ai/runtime/attempt.rs:399-405` (`finish_dispatch`) and `:427-431` (`retry_ambiguous_dispatch`) always move to `Phase::PickCredential { backend: same }`. The backend is excluded only when no untried eligible credential remains (`attempt.rs:304-307`).
- No failure-domain classification feeds the scheduler. Status and transport class reach outlier accounting only.
- The behavior is documented (`docs/en/ai-gateway/resilience.md`, "Credential-first rounds"), but no `skills/` page justifies it for endpoint-scoped failures (grep for credential-first finds nothing). The endpoint-scoped case is the new evidence.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `ai/runtime/attempt.rs`, `ai/runtime/dispatch.rs` and `docs/en/ai-gateway/` are unchanged between feature-08-30 and HEAD.
- `attempt.rs:399-405` (`finish_dispatch`) and `:427-431` (`retry_ambiguous_dispatch`) still move to `Phase::PickCredential { backend: same }` regardless of failure domain.
