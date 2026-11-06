# gw-ai3-01: Guardrail treats "not a guardable SSE response" as a dependency failure — `stream:false` bypasses it under failOpen, and every non-streaming or provider-error response becomes an empty 503 under failClose

- **Category**: design-error
- **Severity**: P1 (correctness/security)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/guards/semantic/plugin.rs:149-188` (`run_upstream_response` applicability gate)
  - `edgion-gateway/src/ai/guards/semantic/plugin.rs:69-94` (`precommit_failure`: fail-open disables the guard, fail-close returns `ErrResponse { status: 503, body: None }`)
  - `edgion-gateway/src/ai/http/proxy.rs:313-329` (AiProxy only rejects Guardrail for native Responses/Messages, not for non-streaming Chat)
  - `edgion-gateway/src/ai/runtime/dispatch.rs:628-633` (semantic source installed regardless of `requested_streaming`)
  - `docs/en/user-guide/http-route/filters/edgion-plugins/guardrail.md:59-62` ("The route must produce a successful, identity-encoded `text/event-stream` response")

## Summary

The response-side Guardrail only knows how to inspect a 2xx identity-encoded SSE stream. Any other
final response — a non-streaming chat completion (`application/json`), or a provider error
(`400 context_length_exceeded`, `401`, `429`, `5xx`) — is routed through the same
`precommit_failure` path that is meant for *dependency* failures (evaluator timeout, saturation).
With `failOpen: true` the guard silently disables itself, so any client defeats the response policy
by sending `"stream": false`. With the default `failOpen: false`, every non-streaming request and
every provider error is replaced by a body-less `503`, destroying the provider's error contract
(including 429 `retry-after`).

## Current behavior (evidence)

`run_upstream_response` (`plugin.rs:157-170`):

```rust
let status = session.get_response_status().unwrap_or(0);
let content_type_ok = ... .is_some_and(is_sse_content_type);
...
if !(200..300).contains(&status) || !content_type_ok || !encoding_ok || !canonical_ai {
    return self.precommit_failure(session, &owner, "response-not-applicable");
}
```

`precommit_failure` (`plugin.rs:69-94`):

- `fail_open == true` → `session.ai().disable_semantic_guard()` and `GoodNext`: the complete
  response is delivered to the client without any evaluator call.
- `fail_open == false` → `ErrResponse { status: 503, body: None }`.

Nothing upstream prevents a non-streaming request from reaching this point when Guardrail is
attached:

- `AiProxy` rejects the Guardrail combination only for `OpenaiResponses | AnthropicMessages`
  (`proxy.rs:313-329`); OpenAI Chat with `"stream": false` is accepted.
- `prepare_ai_attempt` installs the semantic source / usage processor whenever the guard is claimed
  (`dispatch.rs:628-633`), independent of `requested_streaming`, so `canonical_ai` is true and the
  only failing predicate is the content type.

Concrete outcomes on a route with Guardrail attached:

| Client request / provider result | `failOpen: true` | `failOpen: false` (default) |
|---|---|---|
| `stream: false`, provider 200 JSON | **unguarded completion delivered** | empty 503 |
| `stream: true`, provider 400 (context too long) | provider error passes (OK) | empty 503, provider error lost |
| `stream: true`, provider 429 + `retry-after` | passes (OK) | empty 503, no `retry-after` |

The 503 also has no OpenAI-dialect JSON envelope (body `None`), unlike every other AI local error
(`ai/protocols/error.rs:15-22`).

## Industry comparison

- Envoy ext_proc and Kong/APISIX AI content-moderation plugins inspect non-streaming JSON bodies as
  the primary case and streaming as the special case; none treats "no model output to inspect" as a
  dependency failure.
- Provider-side moderation (OpenAI/Azure content filters) annotates or blocks both unary and
  streaming outputs, and never rewrites a provider error into a different status.

## Impact

- Security: operators who choose `failOpen: true` for evaluator availability unknowingly allow any
  client to opt out of the response policy with one request field. This is a client-controlled
  bypass, not an availability tradeoff.
- Availability/compatibility: with the default fail-close, the most common OpenAI call shape
  (non-streaming) is unusable on a Guardrail route, and clients lose actionable provider errors
  (context-length, auth, 429 backoff), which breaks SDK retry logic.

## Suggested direction

1. Split applicability from failure:
   - Non-2xx provider responses contain no model output: pass them through untouched (Guardrail
     stays "not applicable", outcome `skipped`).
   - Non-streaming 2xx Chat responses: either (a) reject at request admission with a fixed
     `UnsupportedParameter` when Guardrail is attached and `stream != true` (same pattern already used
     for native Responses/Messages at `proxy.rs:313`), or (b) add unary support through the existing
     bounded response-head hold (one decision window over the `choices[].message.content` token
     spans).
2. Never let `failOpen` apply to "client chose an unsupported shape"; `failOpen` should only cover
   evaluator dependency failures.
3. Render the fail-close status through `ai_error_response` so the client gets the canonical JSON
   envelope.

Recommendation: (1a) immediately (small, closes the bypass), (1b) later if unary moderation is in
product scope.

## Notes

- Checked `skills/04-review/sse/regression-guards/*` and `skills/04-review/ai-routing/*`; no page
  accepts non-streaming bypass or provider-error masking. The user doc states the route "must
  produce" SSE, but the client — not the operator — chooses `stream`, so the precondition is not
  enforceable by configuration.
- Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `ai/guards/semantic/plugin.rs:154-167`: a non-2xx status or a non-SSE content type routes to `precommit_failure("response-not-applicable")`. `plugin.rs:69-95`: `fail_open` calls `disable_semantic_guard()` and returns `GoodNext`; otherwise it returns `ErrResponse { status: 503, body: None }`.
- `ai/http/proxy.rs:313-329` rejects Guardrail only for `OpenaiResponses | AnthropicMessages`. No code path rejects `stream:false` Chat when `semantic_guard_claimed` is set (grep of `semantic_guard_claimed` / `requested_streaming`).
- The doc (`guardrail.md:59-62`) says the route "must produce" SSE, but the client controls `stream`. Under `failOpen` this is a client-chosen bypass. No review page accepts it.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- The `guards/semantic/plugin.rs` diff only removes the unused `log` parameter from `precommit_failure`; the applicability gate at `plugin.rs:166-170` still routes non-2xx / non-SSE responses to `precommit_failure("response-not-applicable")`.
- `plugin.rs:69-94`: `fail_open` still disables the guard and returns `GoodNext`; fail-close still returns `ErrResponse { status: 503, body: None }`.
- `http/proxy.rs:313-329` still rejects Guardrail only for `OpenaiResponses | AnthropicMessages`; non-streaming Chat is still admitted (`dispatch.rs` unchanged).
