# test-10 — Inventory of assertions that cannot fail

| Field | Value |
|-------|-------|
| ID | `test-10` |
| Disposition | `confirmed` |
| Severity | P3 |
| Dimension | Testing / false green |
| Location | see the table below |
| Reviewer | Test-coverage review |
| Status | open |

## Summary

Nine assertions in the new AI tests are structurally unfalsifiable — they hold for any implementation. None hides a known defect on its own, but together they inflate the apparent coverage of exactly the areas the rest of this batch found problems in.

## Details

| # | Test | Location | Why it cannot fail |
|---|------|----------|--------------------|
| 1 | `assert_paced_protocol` credential stripping | `suites/edgion_ai_service/single_provider.rs:382-395`, `:452-462` | Asserts `auth_is_client_key == false` on a request built with `send_client_authorization: false`, so the header whose absence it checks is never sent. The mock computes the flag by comparing to a header that cannot be present. The sibling `auth_is_provider_key == true` *is* meaningful. |
| 2 | `fixed_error_mappings_do_not_expose_internal_detail` | `ai/error.rs:158-178` | Scans a body assembled entirely from `&'static str` constants for strings no constant contains. The loop then checks only the status code, so swapping the `type_`/`code` pair between two variants passes — 9 of 10 variants have their client-visible `code` unguarded. |
| 3 | `ledger_is_bounded_to_eight_entries_and_eight_kib` | `ai/attempt_runtime.rs:914-936` | The oversized strings are truncated by per-field bounds (317 / 64 bytes) long before the serialized ledger approaches 8 KiB, so the byte-cap branch never runs and the size assertion cannot fail. |
| 4 | `limited_credentials_participate_in_weighted_selection` | `ai/dispatch.rs:978-1013` | Passes `entropy = 0` with the heavy-weight credential at index 0, so the assertion holds for weighted selection, first-eligible selection, or no weighting at all. |
| 5 | `resolves_token_limit_priority_and_default` | `ai/anthropic/request.rs:678-704` | Sets `max_completion_tokens: 42` *and* `max_tokens: 42`, so the assertion is identical whichever field the resolver picks. No test asserts the translated value for a legacy-only `max_tokens` request. |
| 6 | `counters_saturate` (input half) | `ai/usage.rs:858-866` | `set_input_chars` is a plain store and `input_chars()` the matching load — there is no accumulation path, so asserting it equals what was just stored proves nothing. Only the output half exercises `saturating_add`. |
| 7 | Usage shared-state redaction test | `ai/usage.rs:868-882` | Asserts a fixture string is absent from `Debug` of a struct made exclusively of atomics — no rendering could contain it, and only the integer length ever crossed the boundary. The guarantee that matters is `Serialize` of `GenAiSummary`/`GenAiAttempt`, which *do* carry `String` fields, and is unasserted. |
| 8 | `apply_cooldown` 1-second floor | `ai/credential_outlier.rs:342-368` | Only the upper bound (999 s → cap) and zero (rejected) are covered; replacing the clamp with `hint.min(cap)` stays green. The branch is live — the integration fixture returns `x-ratelimit-reset-requests: 1500ms`, and the grammar accepts sub-second values. |
| 9 | `token_quota_downgrade` one-shot claim | `suites/edgion_ai_service/single_provider.rs:1198-1300` | Proves that exhausting `premium-model` re-admits `economy-model`, but never drives the case where the downgrade target's own quota is also exhausted, so the "bounded" claim in the description is unasserted. |

## Impact

Individually minor. Two carry real risk: #8, because a provider returning a sub-second reset hint would install a cooldown already expired by the next admission check — turning passive ejection into a no-op against a provider that just asked the gateway to back off; and #7, because a future field echoing prompt text or credential material into `GenAiSummary` would pass the redaction test unchanged.

## Suggested fix

1. Route `assert_paced_protocol` through the `send_client_authorization: true` variant (the flag is already threaded).
2. Replace the substring scan with an exhaustive `(error, status, type, code)` table over all ten variants plus `assert_eq!(body["error"].as_object().unwrap().len(), 3)`; move the leakage concern to a test over the path that actually carries provider text.
3. Either assert the byte cap directly via a `#[cfg(test)]` seam on the per-field bound, or rename the test and note in a comment that the byte cap is unreachable defence-in-depth given the field bounds.
4. Place the heavy-weight credential at index 1 with weights `[1, 100]` and entropy `5`, asserting credential 1 wins.
5. Replace the equal-value case with a legacy-only request asserting the translated value, and add a request supplying only `max_completion_tokens` alongside a different service default.
6. Drop the `input_chars` assertion or replace it with a real accumulation check.
7. Build a `GenAiSummary`/`GenAiAttempt`, serialize it, and assert the key set and the absence of body/prompt/credential material.
8. `assert_eq!(credential.apply_cooldown(Duration::from_millis(1)), Some(Duration::from_secs(1)));` followed by an admission assertion.
9. Issue a third request after `economy-model` is itself exhausted and assert a single 429 with no second downgrade header.
