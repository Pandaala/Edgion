# Implementation Record

## Delivered

- Added `AiProvider::Anthropic`, its default endpoint, optional
  `defaults.maxOutputTokens`, 512-byte model-name/alias bounds, CRD coverage, controller wiring,
  and lock-free runtime compilation.
- Added a strict, bounded OpenAI Chat Completions to Anthropic Messages request converter for
  text, tools, tool results, stop sequences, streaming, and explicit token limits.
- Added total response header/status normalization, fixed redacted provider errors, bounded
  non-streaming conversion, and a strict serial SSE translator for text, tool fragments, finish
  reasons, cache-inclusive usage, and OpenAI `[DONE]` framing.
- Extended the private response processor seam with a default header callback and body-length
  declaration. Edgion removes stale `Content-Length` at both response-header boundaries; no
  Pingora fork change was made.
- Kept actual per-request usage in the access log. The optional process-local metric remains
  disabled by default, has no model label, and has a fixed maximum of 12 series after adding the
  Anthropic provider.
- Added English and Chinese user documentation, resource architecture references, and an
  isolated Anthropic integration suite.

## Review convergence

An independent design review was resolved before implementation. A separate independent code
review then found five issues; all were fixed:

- Anthropic now rejects unknown-model passthrough and bounds catalog names/aliases.
- The shared SSE decoder exposes sticky malformed-input and idle-state signals so the strict
  translator fails closed without changing tolerant observational consumers.
- Non-empty initial message/block content is rejected instead of being silently discarded.
- Provider-specific Anthropic and AWS request/rate-limit headers are stripped.
- `message_stop` is terminal and EOS requires no incomplete event state.

## Verification

- Anthropic request/response unit tests: 29 passed.
- Shared SSE decoder unit tests: 18 passed.
- AiProxy unit tests: 20 passed.
- EdgionAIService unit tests: 12 passed; CRD coverage: 9 passed.
- Anthropic live integration:
  `tmp/integration_testing/testing_20260803_222947/report.log` — passed.
- AI-1a SingleProvider regression:
  `tmp/integration_testing/testing_20260803_223023/report.log` — passed.
- Workspace format, compile, clippy, agent-doc, SSA, metrics inventory, and Pingora source-policy
  checks passed. Clippy continues to report the repository's existing warning backlog.

## Residual limits

- An unknown-length non-streaming overflow discovered after downstream status commit can only
  replace the body; it cannot revise the committed status.
- A failed SSE translation is sealed and drained, but upstream response termination remains an
  AI-4/Pingora-fork capability decision.
- Anthropic-specific Secret rotation does not duplicate the existing dynamic scenario: the
  dependency, resolver, and hot-update path is shared, while the Anthropic integration verifies
  the resolved value is installed as `x-api-key`.
