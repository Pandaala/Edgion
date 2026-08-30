# Conflicting OpenAI output bounds are client errors

OpenAI-compatible requests may use the current `max_completion_tokens` field
or the deprecated `max_tokens` field. When both are present, equal values are
accepted; different values make the request ambiguous and must be classified
as `AiDispatchError::Client(AiProxyError::MalformedBody)`.

Do not classify this conflict as `InvalidConfiguration`. That variant is for
operator or internal dispatch state and falls through to the generic gateway
500 path. A client-controlled conflict must instead use the canonical
OpenAI-compatible JSON 400 envelope so SDKs can parse it and server-error SLOs
remain accurate.

All consumers of the request output bound continue to use
`request_output_bound`; conflict validation belongs in
`effective_max_output_tokens`, where dispatch turns the canonical request into
the selected attempt's quota and cost inputs.
