---
name: replay-body-lifetime
description: Review AI attempt-plan body retention across retries and final HTTP responses.
knowledge_type: regression-guards
---

# AI replay body lifetime at final writer claim

AI dispatch reads the canonical `ReadyAiAttemptPlan.request_body` to build
provider requests across retries. A candidate response can still be replaced.
On a successful `EdgionHttpContext::claim_final_response`, the final writer
sets `response_write_started` and releases the plan's body reference. The AI
status and transport retry gates both consult this flag. Plan metadata stays
available during response processing and settlement.

In gw-ai3-07 the plan kept its bytes until request teardown, extending their
lifetime during long SSE responses. A storage-backed body can keep a budget
lease alive. Usage injection can instead leave a separately allocated JSON
body in the plan. The locked Pingora HTTP/1 and HTTP/2 implementations release
their registered replay buffer once the final header is committed and replay
is ready or done. The plan alone does not imply that every stream pins a spill
file or multiple budgeted copies. See the
[storage budget guard](../../http-body/regression-guards/request-body-aggregate-budget.md).

`final_writer_releases_ai_body_after_the_retry_window` checks the body owner
across status retry, candidate selection, an informational status and final
writer claim. All 14 Gateway library tests in `ai::http::attempt::tests`
passed, including this owner-lifetime check. Recheck this boundary if a retry can begin after writer claim or
a response consumer starts reading the canonical request payload.
