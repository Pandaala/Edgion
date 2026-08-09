# `genai.model` is bounded but may be client-controlled

The access-log `genai.model` field preserves the effective logical model used
for dispatch. It is normally the requested declared model name or alias; a
quota downgrade replaces it with the configured fallback. When unknown-model
passthrough is enabled, the value may instead be client-controlled, with the
request parser enforcing a 512 UTF-8 byte limit.

This is an intentional request-level access-log dimension, not a Prometheus
label. Operator documentation must call out its possible cardinality and tell
downstream consumers to bucket it when they need a bounded facet. The GenAI
token metric remains limited to fixed enum labels and deliberately omits model.

Do not introduce a synthetic `other` bucket solely to make the access-log field
catalog-bounded: that would discard useful request identity without changing
the in-process metric surface. Reconsider bucketing only if the product adopts
a separate explicit low-cardinality logging field.
