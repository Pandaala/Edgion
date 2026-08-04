# Decisions and Reviews

## Initial decisions

- Anthropic-first only; cloud providers, Bedrock, and multi-service picking are deferred.
- `defaults.maxOutputTokens` is optional and explicit. Missing request and service values fail
  locally; Edgion does not invent a provider token limit.
- Unsupported or unknown OpenAI fields fail locally. No vendor-options extension is introduced.
- Provider-native errors are mapped to fixed categories; raw messages and request IDs are not
  forwarded or logged.
- Streaming conversion failure emits one fixed OpenAI error and seals the translator. Raw
  Anthropic bytes are never a fallback.

## Design review

Independent read-only review completed after the initial design. All Must Fix findings were
adopted:

- Added a default response-header callback and body-length declaration to the private streaming
  processor contract. Header normalization runs after retry arbitration; framing is enforced at
  the end of both response-header phases.
- Replaced the partial status discussion with a total HTTP/media-type/content-encoding matrix
  and explicit provider-header stripping.
- Made the Anthropic processor publish exact normalized usage to the existing shared usage state,
  independently of client-visible usage-chunk emission.
- Replaced the impossible multi-active-block model with Anthropic's serial, consecutive,
  single-active-block lifecycle and explicit identifier/block limits.
- Added distinct retained-input and generated-output caps, including per-callback SSE output.
- Added the executable field/type/null/range/tool-graph request matrix and canonical OpenAI wire
  examples in `04-wire-contract.md`.
- Added one bounded `AiResponseTranslationFailed` request status emitted exactly once on response
  translation failure.

Nice-to-have decisions:

- TTFT remains first upstream body byte to preserve AI-1a. First client-visible-token timing is a
  separate future semantic.
- Streaming `partial_json` stays opaque and is not accumulated; only its SSE/JSON envelope is
  validated. Complete tool arguments therefore remain provider-owned.
- Adversarial provider fixtures for wrong status, media type, encoding, identifiers, and event
  mismatch were added to the test plan.

Assumptions rechecked against the code:

- Edgion implements no Pingora response-cache callbacks, so the response cannot be translated
  twice from cache.
- Request-body overwrite publishes through `BodyStorageView::publish_arc`, which enforces the
  resolved physical cap atomically.
- Bounded non-streaming response state uses the existing private processor seam and introduces no
  response `BodyStorage` or uncapped read.
- H1 framing needs `Content-Length` removed by the end of `upstream_response_filter`; the second
  removal after async response plugins is a final CL+TE/H2 metadata guard.

Accepted interim risks:

- An unknown-length non-streaming overflow is discovered after status commit, so the fixed error
  body retains the already-committed status. Raw provider bytes are still never forwarded.
- A sealed SSE translator cannot terminate the upstream read until AI-4. It remains bounded,
  outputs one fixed error, and drops later bytes.

## Code review

The independent post-implementation review was resolved in full. It tightened model catalog
bounds and Anthropic catalog matching, malformed UTF-8 handling, initial-content validation,
provider-header stripping, and terminal SSE state. No unresolved correctness or security finding
remains; output-boundary and H1/H2 behavior are covered by unit plus live integration tests.
