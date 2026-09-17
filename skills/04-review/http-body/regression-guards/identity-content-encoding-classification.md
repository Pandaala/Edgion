---
name: identity-content-encoding-classification
description: Use when reviewing response-body consumers that require identity Content-Encoding, especially Guardrail response admission and first-head Hold gating.
knowledge_type: regression-guards
---

# Identity response encoding classification is shared across semantic consumers

## Conclusion and scope

Response-body consumers that require an identity-coded representation use
`response_is_identity_encoded` from
`routes/http/proxy_http/content_type.rs`. The classifier accepts an absent
`Content-Encoding` field or a field set whose every comma-delimited coding is
case-insensitively `identity`. It examines every repeated field line without
copying header values. Per RFC 9110 section 5.6.1.2, empty list members are
ignored; a present field with no effective coding, a non-UTF-8 value, or any
other coding is rejected.

The shared consumers are Guardrail response admission, Guardrail's final
response-head Hold gate, and Anthropic response translation. This rule does
not authorize decoding an upstream response or buffering its complete body.

## Rationale and evidence

The response-body streaming invariant in the repository-root `AGENTS.md`
requires the main upstream response to remain streaming. Guardrail accepts only
identity-encoded canonical AI SSE, as documented in
`docs/en/user-guide/http-route/filters/edgion-plugins/guardrail.md`.

Historically, `Guardrail::run_upstream_response` and
`response_head_commit_plan` each used a first-value lookup followed by one
whole-string comparison. A repeated header whose first value was `identity`
could therefore hide a later `gzip`, while the valid list
`identity, identity` was rejected. Anthropic response translation already had
the required all-field/all-token behavior, but as a private local helper.

The correction moves that established behavior into one borrowed `HeaderMap`
classifier. `Guardrail::run_upstream_response` calls it through
`PluginSession::response_header_map`, the final Hold gate calls it on the final
`ResponseHeader`, and Anthropic translation delegates to the same helper.
This preserves the existing failure policies and final-head eligibility
ownership while preventing the two Guardrail gates from drifting apart.

## Alternatives and limits

Adding another owned multi-value session method is unnecessary because the
live borrowed response map already exposes every field value. Extending
`FinalResponseEligibility` with content encoding is also unnecessary: that
type owns method, status, and SSE stream-kind facts used outside Guardrail.

The classifier ignores empty list members as required for HTTP recipients, but
still rejects a present field containing only empty members because it does not
establish an identity coding. It does not parse transfer codings, negotiate
encodings, decompress payloads, or change request-side JSON admission, which
intentionally requires one exact identity value. Main-response body processing
remains bounded and streaming.

## Verification and re-evaluation

The focused unit test
`identity_encoding_classifies_every_field_and_list_member` passed. It covers
absent and single identity values, repeated identity fields, comma-combined
identity values, case and optional whitespace, mixed identity/gzip orderings,
ignored empty members, all-empty field sets, and a non-UTF-8 value.

The focused HTTP suite `EdgionPlugins/GuardrailEncoding` covers both Guardrail
failure policies on the wire. It sends repeated `Content-Encoding` fields
`identity` then `gzip` on canonical AI SSE, asserts fail-open passthrough and
fail-close `503`, requires zero decision-service calls, and checks the one
aggregate `reason=response-not-applicable` summary. The existing
`EdgionPlugins/Guardrail` suite was left unchanged.

Re-evaluate if HTTP field-list parsing policy changes, Pingora begins decoding
upstream response bodies before these consumers, `PluginSession` response-map
ownership changes, or another semantic response-body consumer requires an
identity-coded representation.

## Resolution references

- `D0105` / `unify-response-content-encoding-classification` — P2 bugfix,
  corrected in 2026-09.
- `D0105 follow-up` / `guardrail-content-encoding-failure-policy-integration` —
  wire-level fail-open/fail-close coverage added in 2026-09.
