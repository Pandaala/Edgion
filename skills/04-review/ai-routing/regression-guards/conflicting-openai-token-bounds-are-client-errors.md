---
knowledge_type: regression-guards
---

# Conflicting OpenAI output bounds are client errors

OpenAI-compatible requests may use the current `max_completion_tokens` field
or the deprecated `max_tokens` field. When both are present, equal values are
accepted; different values make the request ambiguous and must be classified
as `AiProxyError::MalformedBody` during ingress metadata capture. Dispatch clients
retain the `AiDispatchError::Client` envelope for client errors.

Do not classify this conflict as `InvalidConfiguration`. That variant is for
operator or internal dispatch state and falls through to the generic gateway
500 path. A client-controlled conflict must instead use the canonical
OpenAI-compatible JSON 400 envelope so SDKs can parse it and server-error SLOs
remain accurate.

All consumers of the request output bound continue to use
`openai::request::prepare` and its private `metadata_from_fields` helper; the
test-only `metadata` adapter delegates to that same helper. Conflict validation
runs during selective request metadata capture, before selection or dispatch. The plan stores one resolved bound used by CostWeighted and credential reservation.

## Resolution references

- `gw-02` — 2026-08-09.

## Task 05 owner update

The production output-bound owner is now `ai/protocols/openai/request.rs`. Selective ingress
metadata validates conflicting numeric spellings as a fixed client 400 and captures one bound
for both selection and reservation. Native forwarding retains unknown extension bytes;
no whole-document Value deserialization is required for this metadata. Private test helpers
in dispatch only delegate to this parser. Fresh Task 05 verification is recorded in its task.
