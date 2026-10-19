---
name: webhook-read-then-truncate-16kb-cap
description: Use when reviewing webhook response buffering; withdraws the former read-then-truncate argument and records current streaming-cap evidence and its memory limits.
knowledge_type: review-evidence
---

# Webhook response buffering: streaming-cap evidence

## Corrected conclusion

The former page argued that reading a complete response with `bytes().await` and
then truncating it to 16 KiB bounded the extra memory read to 16 KiB. That argument
is invalid: a limit applied after full collection does not bound collection-time
memory. Neither a configured maximum nor two `min()` expressions prove that the
reader enforces the maximum while receiving. The former "minimal impact; no fix
needed" verdict must not suppress a finding about an uncapped reader.

This correction does not reconstruct the historical implementation or reproduce
a 1 GB response. It withdraws the flawed reasoning and describes current code at
`3437325ba`; current correctness does not validate the historical claim.

## Current source evidence

Both `resolve_webhook_key` and `WebhookCallHandle::call` use the shared
`outbound_call` with `get_webhook_global_max()`. The global atomic starts at
16,384 bytes but is configurable, not a fixed 16 KiB hard ceiling. Gateway config
recomputation selects the minimum configured webhook response cap, falling back
to the schema default when none is present.

`outbound_call` computes the effective maximum as the minimum of that supplied
global value and the prepared client's max_response_bytes. A response selected
for a status retry can be dropped without body collection. For a response whose
body must be read, the implementation:

1. Rejects an advertised Content-Length exceeding the effective maximum.
2. Passes the byte stream to `stream_body_chunks_capped`.
3. Treats the collector's overflow flag as `OutboundCallError::ResponseTooLarge`,
   recording the existing passive-health failure rather than returning a
   successful truncated response. A stream read error remains a call failure.

The streaming collector copies at most the remaining allowance from each chunk
and stops polling on the first overflowing chunk. At an exact boundary it must
still observe EOF or another chunk to distinguish a complete body from overflow.
Content-Length is an early rejection optimization; streaming enforcement is the
load-bearing bound for missing or inaccurate lengths.

## Scope of the memory guarantee

The effective cap bounds logically accumulated body bytes. It does not exactly
bound allocated Vec capacity, a transport chunk already delivered to the collector,
network-library buffers or total process heap. Do not infer a precise peak-memory
number from the logical cap alone. Webhook remains a separate bounded outbound
exchange; this is not permission to aggregate the main proxied response.

Re-evaluate if a caller bypasses the collector, accepts its overflow prefix as a
successful webhook response, moves enforcement after a full read, or changes cap
publication. Preserve the canonical shared reader rather than adding another
webhook-local buffering implementation.

## Verification and sources

Fresh verification passed the four network-free collector tests for empty/zero/
exact EOF, first-overflow polling termination, read errors and cancellation drop.
Those tests establish collector behavior; they do not measure heap/RSS, exercise
live transport, or prove the historical claim. The outbound overflow-to-error
mapping above was inspected in current source.

- `edgion-gateway/src/link_sys/providers/webhook/resolver.rs`
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs`
- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`: recompute_global_config
- `edgion-gateway/src/plugins/http/common/outbound/call.rs`
- `edgion-gateway/src/util/http_body.rs`
- `edgion-gateway/tests/unit/util/http_body_test.rs`
