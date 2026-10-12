# AI future work

One independent task per Markdown file. All tasks are pending future selection;
creating this backlog authorizes no implementation or paid calls. Optional/deferred
features are not automatic prerequisites for completing the existing native gateway.
Each task contains its own scope, boundaries, acceptance and verification context.
Dependencies refer only to tasks in this directory or the existing implementation.
Project rules and current knowledge/code are authoritative and must be revalidated.

Completed native implementation, local SDK coverage, the first scoped performance
run and normal subscription-client conversation checks are not reopened here. Two
verification tasks preserve only their explicitly described remaining evidence gaps.

| File | Goal | Tier |
|---|---|---|
| [native-gemini.md](native-gemini.md) | Add managed Gemini native access | Deferred |
| [bedrock-endpoint-authentication.md](bedrock-endpoint-authentication.md) | Add a selected Bedrock endpoint and authentication profile | Deferred |
| [chat-anthropic-compatibility.md](chat-anthropic-compatibility.md) | Complete the supported Chat-to-Claude conversion subset | Optional |
| [chat-gemini-conversion.md](chat-gemini-conversion.md) | Add the limited Chat-to-Gemini route | Deferred |
| [chat-bedrock-conversion.md](chat-bedrock-conversion.md) | Add the limited Chat-to-Bedrock route | Deferred |
| [trusted-identity-and-model-access.md](trusted-identity-and-model-access.md) | Bind trusted identities to AI model and provider access | Optional |
| [request-output-ceilings.md](request-output-ceilings.md) | Enforce explicit per-request output ceilings | Optional |
| [shared-and-hierarchical-token-budgets.md](shared-and-hierarchical-token-budgets.md) | Support explicit shared budget identities and independent quota dimensions | Optional |
| [quota-modes-and-inflight-reservations.md](quota-modes-and-inflight-reservations.md) | Define low-cost cumulative quotas and optional inflight reservations | Optional |
| [weighted-and-monetary-budgets.md](weighted-and-monetary-budgets.md) | Add optional weighted or monetary budget enforcement | Optional |
| [native-guardrail-capabilities.md](native-guardrail-capabilities.md) | Extend guards through explicit native protocol capabilities | Optional |
| [budget-operations-and-reconciliation.md](budget-operations-and-reconciliation.md) | Expose budget state and reconciliation outcomes for operators | Optional |
| [live-provider-api-verification.md](live-provider-api-verification.md) | Verify the declared managed AI APIs against real providers | Future verification |
| [production-ai-performance-verification.md](production-ai-performance-verification.md) | Establish a production AI cost and capacity baseline | Future verification |
