# AI quota throttle must wait and readmit once

## Decision

`tokenQuota.onExceeded: Throttle` is a bounded one-shot policy. An exhausted
initial admission waits for `min(retry_after, action_budget)`, then performs at
most one more admission within the remaining policy and request budget. The
remaining request budget is sampled after the initial admission, immediately
before the throttle action begins, so time spent waiting on Redis cannot be
reused as sleep budget. A second exhaustion, timeout, or expired request
deadline uses the ordinary quota rejection response.

The orchestration belongs to `CompiledAiQuotaPlan::admit`. Its raw Redis
admission is module-private, so AiProxy cannot accidentally bypass the frozen
Reject/Throttle/Downgrade action. Downgrade still performs its explicit second
admission only after changing the logical model.

## Regression coverage

Paused-time unit tests inject a scripted admission source. One requires an
exhausted first call with a 40 ms retry delay to wait exactly 40 ms and return a
second-call bypass. Another makes the second call exhausted and makes any third
call panic, proving that the second exhaustion is returned instead of entering
a retry loop. Removing the sleep, removing readmission, or turning the policy
into a loop each fails independently without a real Redis instance or
wall-clock tolerance.

The integration fixture's final 429 remains useful protocol coverage, but it is
not used to prove the wait because that would introduce timing-sensitive CI.
