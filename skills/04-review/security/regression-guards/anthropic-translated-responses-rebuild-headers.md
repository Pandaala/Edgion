---
name: anthropic-translated-responses-rebuild-headers
description: Use when reviewing Anthropic response-header forwarding, provider metadata exposure, or headers synthesized after response translation.
knowledge_type: regression-guards
---

# Anthropic translated responses rebuild their headers

The Anthropic response processor translates both successful and error bodies into an
OpenAI-compatible representation. Provider headers therefore are neither authoritative for the
new representation nor part of the public response contract. The processor clears the complete
upstream header map and then emits only:

- the synthesized `Content-Type`; and
- `Cache-Control: no-store`, because model output is request-specific and may contain sensitive
  data; and
- a syntactically valid `Retry-After` on the fixed 429 response.

This is intentionally an empty-base policy rather than a denylist. Provider, CDN, and compatible
endpoint headers can evolve without exposing request identifiers, routing metadata, cookies,
redirects, validators, framing, or implementation fingerprints to downstream clients.

Edgion-owned headers are not preserved through the provider boundary. They are applied afterward:
the proxy response pipeline applies configured server headers, tracing and session-persistence
headers run after translation, and response plugins may add their own mutations. This ordering
keeps ownership explicit and prevents an upstream from spoofing a gateway-owned header.

Do not add a provider header to the translated contract merely because a particular upstream uses
it. A new downstream-visible header needs a gateway-owned semantic definition, validation, and a
test that proves provider input cannot control an invalid value.

## Resolution references

- `gw-19` — 2026-08-09.


## Retry-After ownership decision

User-directed value review at `895a8efd1` declined response-07's standalone
HeaderValue-transfer optimization. Keep the validated text snapshot before the
provider-header reset. Only original 429 responses take it; unary values are
limited to 64 raw bytes, while streaming has no equivalent 64-byte bound. This
is not a claim that every accepted value is short or that 429 traffic is rare.

The pinned HTTP/1 parser slices values from a shared receive allocation. Moving
one field can retain that larger owner; byte/pointer parity alone does not prove
better memory use. The withdrawn prototype's verification remains historical in
the data-plane review INDEX. There is no correctness fix here, and evaluating
transport lifetimes is disproportionate to the currently evidenced copy saving.
Reopen only with measured translated-429 copy cost and real backing-lifetime
results, including long accepted streaming values. Preserve first-value policy,
validation, sensitivity metadata and the empty-base security contract.
