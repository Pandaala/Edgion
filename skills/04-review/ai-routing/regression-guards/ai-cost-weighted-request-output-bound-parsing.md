---
name: ai-cost-weighted-request-output-bound-parsing
description: Use when reviewing AI backend-selection or token-accounting findings; covers why every consumer of the client-declared output-token bound must resolve it through one shared parser that prefers max_completion_tokens.
status: fixed
finding_id: gw-01
closed: 2026-08-04
knowledge_type: regression-guards
---

# gw-01 — CostWeighted read only `max_tokens` — FIXED

## Conclusion

Every consumer of the *client-declared* output-token bound in a canonical OpenAI
request body must resolve it through `request_output_bound`
(`edgion-gateway/src/ai/dispatch.rs`), which prefers `max_completion_tokens` and
falls back to the deprecated `max_tokens`. `request_max_tokens` previously read
only `max_tokens`, so a body from a current OpenAI SDK looked bound-less to
`CostWeighted` selection while the sibling TPM helper saw the real value.

Fix suggestions of the form "make `defaults.maxOutputTokens` mandatory" or
"have `pick_cost_weighted` fall back to a synthetic bound when none is declared"
are **not accepted**.

## Core Rationale

**1. The two readers disagreed about the same body**

`request_max_tokens` (feeds `PickerInput::request_max_output_tokens` before
backend selection) read `max_tokens` only. `effective_max_output_tokens` (feeds
TPM reservation after selection) and Anthropic's `resolve_request_max_tokens`
(`ai/anthropic/request.rs`) both read `max_completion_tokens` first. Current
OpenAI clients send `max_completion_tokens`; `max_tokens` is deprecated for chat
completions. A split parser means cost ranking and quota accounting disagree
about what the client asked for — that divergence, not the field name, is the
defect.

**2. The failure was a fail-closed 500 with a non-JSON body**

`pick_cost_weighted` (`ai/routing.rs`) takes
`request_max_output_tokens.or(service_max_output_tokens)` and returns
`PickerError::MissingCostInput` when both are absent.
`defaults.maxOutputTokens` is optional — `EdgionBackend::validate` only forces
it when a credential declares `tokensPerMinute`. So the legal combination
"`backendSelection: CostWeighted` + no service default + a client sending only
`max_completion_tokens`" produced `AiDispatchError::InvalidConfiguration`, which
`ai_predispatch_error` maps to `None` and therefore never reaches
`render_openai_error` — the client got Pingora's default HTML 500 instead of an
OpenAI-shaped JSON error. Renaming the field to `max_tokens` made the identical
request succeed.

**3. Even with a service default the ranking was wrong**

When `defaults.maxOutputTokens` was set the request did not 500, but cost
ranking used the service ceiling instead of the (usually much smaller) client
bound, skewing `CostWeighted` away from its documented contract: the output
bound comes from the request first, the service model entry second.

## Fix Suggestions Not Accepted

- Make `defaults.maxOutputTokens` mandatory whenever `CostWeighted` is selected — the field is a *fallback*; forcing it hides the parser bug and breaks configurations that legitimately let the client set the bound.
- Have `pick_cost_weighted` substitute a default bound when none is declared — `MissingCostInput` fail-close is deliberate (a silent default would make cost ranking fiction); the bug was that a declared bound was not being seen.
- Duplicate the `max_completion_tokens.or(max_tokens)` expression at the third call site instead of extracting a shared helper — a fourth reader would drift the same way.
- Fold the `max_tokens` / `max_completion_tokens` *conflict* rejection into the shared parser — that check is `effective_max_output_tokens`-local and its `InvalidConfiguration` → HTML 500 mapping is tracked separately as `gw-02`.

## Re-evaluation Triggers

Re-open this decision only if:

- OpenAI introduces a third spelling for the request output bound, or reverses the deprecation of `max_tokens`.
- A consumer appears that genuinely needs the raw per-field values rather than the resolved bound (today only the conflict check does, and it reads them directly).

## Reference Cases

- `tasks/review-ai-work/gw-01-costweighted-ignores-max-completion-tokens.md` (AI dispatch review, 2026-08-04)
- `edgion-gateway/src/ai/dispatch.rs` — `request_output_bound`, `request_max_tokens`, `effective_max_output_tokens`; regression tests `request_max_tokens_prefers_max_completion_tokens` and `cost_weighted_accepts_max_completion_tokens_only_body_without_service_max`
- `edgion-gateway/src/ai/routing.rs` — `pick_cost_weighted` output-bound resolution
- `docs/en/ai-gateway/routing.md` / `docs/zh-CN/ai-gateway/routing.md` — operator-facing statement of where the bound comes from

## Resolution references

- `gw-01` — 2026-08-04.
