# Evaluate selective request-guard refusal facts

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `02154ff52`.

## Evidence and decision

AiGuard::refuse calls request_facts only for contentFilter replies. The helper
parses the complete canonical request into serde_json::Value to select model and
stream, discarding all other fields. It runs after policy rejection or a fail-close
dependency outcome, including requests that span selection could not parse.
The decoded model already moves out of the tree; its former copy is removed.

A selective visitor could avoid materializing unrelated request content, but a
simple replacement changes observable refusal rendering. Current full-parse
failure defaults both facts to empty model and non-streaming. Successful object
parsing uses the last duplicate key and independently defaults wrong field types.
A derived two-field struct instead rejects duplicate selected keys and can accept
ignored values rejected by Value. A standalone probe against serde_json 1.0.151
with raw_value and serde 1.0.229 found:

| Input alongside model="m", stream=true | Value | Derived selective facts |
| --- | --- | --- |
| ordinary ignored number | accepted | accepted |
| ignored number 1e9999 | rejected | accepted |
| ignored value with 130 nested arrays | rejected | accepted |
| duplicate model keys (first/last) | accepted, last wins | rejected |

The probe tests dependency behavior, not an implemented Gateway candidate.
IgnoredAny also does not establish complete UTF-8 validity of ignored strings;
see native-messages-preservation-and-accounting.md. The locked ignore_value /
ignore_integer / ignore_str paths differ from typed Value decoding. A UTF-8 scan
alone does not establish equivalent numeric, depth or escaped-string validation.

Decide whether reject-path allocation/CPU measurements justify a reviewed
selective mode with equivalent validation, or whether a deliberately different
refusal-facts policy is desired. No performance gain has been measured and no
security defect is asserted. This does not claim all selective visitors are
incapable of preserving the existing behavior.

## Required boundaries

Assess existing ai/protocols/json_span and native metadata parsing owners before
adding functionality. Their ambiguity, depth and numeric policies are not assumed
to match request_facts. Reuse a suitable canonical mechanism; do not add another
body cache or unrelated JSON parser. Do not treat successful span selection as a
prerequisite: refusal also handles unsuccessful selection and unavailable bodies.

Preserve complete syntax/trailing-input, UTF-8, escaped-string/surrogate, numeric
and depth validation, root-type behavior, last-key selection and both-facts
fallback on parse failure unless a specific semantic change is approved. Keep
wrong-type defaults, the canonical renderer's final 128-Unicode-scalar model
truncation and escaping, content type, HTTP status and exact SSE framing. Retain
BodyStorage ownership, failure-policy and generation/order gates, and add no
request logs or metrics.

## Acceptance for a later implementation

Compare CPU and allocations for small and large rejected bodies, many unrelated
fields, escaped models, deep inputs and failure-policy refusals. Differentially
compare facts and final reply dialect against the current helper for malformed
ignored values, duplicate selected/unselected keys, wrong types, root scalars,
trailing input, numeric overflow, invalid UTF-8 and surrogate/depth boundaries.
Any accepted semantic difference must be explicit, not hidden as an optimization.

Use IMPL with an exact allow-list and independent review. Relevant authorities:
`skills/04-review/plugin-runtime/regression-guards/ai-guard-span-bounds-and-skip-semantics.md`,
`skills/04-review/plugin-runtime/regression-guards/ai-guard-ordering-publication.md`,
and `skills/04-review/ai-routing/regression-guards/native-messages-preservation-and-accounting.md`.
