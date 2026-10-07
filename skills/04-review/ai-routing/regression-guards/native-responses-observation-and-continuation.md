# Native Responses observation and continuation

Task 06 enables POST /v1/responses for api=openaiResponses and provider OpenAI. No conversion,
background creation, management operations or native guards. Protocol selection comes from config
and effective path, never authentication headers or body guessing.

Native request controls follow the provider's nullable schema: explicit null for background and
stream selects their false defaults; null max_output_tokens leaves the bound unspecified. Preserve
these values in the forwarded native document and during selective model rewriting. Non-null
invalid types remain rejected, as does background=true. The official OpenAI Python
[Responses request schema](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_create_params.py)
declares these controls optional and nullable. The request mirror's nullable-control regression and
the EdgionBackend/Responses nullable_controls live case cover parsing and byte preservation.

The canonical selective JSON tokenizer optionally captures short selected strings under a decoded
byte cap; default consumers still discard string values. Native Responses output is not aggregated.
JSON/SSE usage shares the existing observation and quota owners, with reported-only actual billing.
SSE terminal type/status must agree; partial EOS tails, oversized events or malformed documents
invalidate numeric accounting. JSON cancelled is terminal; no response.cancelled SSE is assumed.
Completed without usage can provide a success fact while billing stays unknown; failed/incomplete/
cancelled never teach credential health or adaptive TTFT success. Off installs no parser and permits
transport latency only. Later transport truncation retains reservation regardless of an earlier total.

Continuation checks raw backend slots and raw credential bindings before eligibility filters.
Controller preserves declarations; Gateway records declared_credential_count independently of the
usable pool. Zero-weight backend slots count; credential weight zero is invalid configuration.
Require one OpenAI backend and one binding, unchanged model, no modelRoutes, no quota downgrade,
and zero retries even after global cap initialization. No affinity registry is added; Secret principal
rotation remains an operator obligation. Disabled/condition-skipped/policy-bypassed guards do not
set the allocation-free invocation marker; actual pre-rewrite guards do.

Re-evaluate when adding native guard capability, backend/principal affinity, new typed terminal
outcomes, request estimate semantics or provider event changes. Focused tests live under Gateway
AI mirrors, Resources plugin validation, WAF selected JSON mirrors and EdgionBackend/Responses.

Provider principal selectors OpenAI-Organization/OpenAI-Project are stripped by canonical
credential installation, after backend modifiers, for all managed AI requests. Shared RequestId
reserved-target validation also rejects them to prevent later reconcile_request reintroduction.
A one-binding check alone does not isolate a legacy key with caller-controlled project selectors.
Use fixed operator-owned principals/project-scoped keys; no selector override config is introduced.

Responses request validation checks the complete byte slice with `from_utf8` before
selective serde deserialization. Skipping ignored string fields does not establish
UTF-8 validity in the locked serde_json visitor. Invalid input/extension strings are
MalformedBody (400) before observation, model rewrite or dispatch, including nested
ignored values. Parser and canonical-request unit regressions and the Responses
`malformed_utf8` shared-topology case cover off/provider observation and unchanged/
aliased models. The new live case awaits fresh parent acceptance for this correction.
