---
name: native-messages-preservation-and-accounting
description: Native Messages header isolation, binding fences, bounded reported-only observation and native error ownership.
knowledge_type: regression-guards
status: implemented
---

# Native Messages preservation and accounting

## Required behavior

Native anthropicMessages is independent of explicit Chat-to-Anthropic conversion.
It accepts POST /v1/messages, observes only when requested, and keeps original
native bytes/query/version/repeated beta values. No response BodyStorage, full
document, disk spool, conversation AST, affinity registry or second accounting
service is permitted. The converter remains the sole parser of converted traffic.

The canonical connector installs the Secret key after backend modifiers, clears
authentication and workspace selectors, and rejects effective profile attribution.
RequestId reserves Anthropic protocol/identity targets; native RemoteIp collisions
are rejected through the existing session interface. Deferred version mutations
are resolved before observation admission. Only version 2023-06-01 is certified;
optional unsupported-version observation remains sticky unknown and required
accounting rejects before provider dispatch.

State-bound container/file/thinking history requires one raw backend slot and one
declared credential with an unchanged provider model and no model routes,
downgrade or retry. Inspect file references inside tool_result content under
independent depth/item bounds. Unresolved credentials and zero-weight backend
slots count; a declaration does not prove cross-request principal identity.

Provider consumption is reported-only. Retain raw cumulative input/output/cache
components, use checked cache-inclusive normalization and existing watermarks.
Use the actual JSON/SSE envelope to select metadata; an opaque extension named
message/delta cannot replace usage. Selected duplicates replace all state owned
by that value, including iterations attribution. Malformed iteration models and
unsupported inference yield unknown, never an authoritative top-level subtotal.
Same-model message iterations are summaries and are not added again.

Explicit null required counters/object parents invalidate observation; nullable
delta input/cache patches and stop_reason remain permitted. Duplicate keys retain
same-document last-wins semantics. Recognized SSE envelopes require matching
payload types; unknown event names cannot contribute lifecycle or usage metadata,
but remain subject to canonical decoder poison/length/framing limits.

Start placeholders are insufficient for final SSE usage. Require validated
message_delta output, valid stop metadata and message_stop, and preserve actual
source EOS/delivery distinctions. Normal tool_use/pause_turn/max_tokens/refusal
stops are successful message completion. A native error latches provider failure;
a later message_stop cannot overwrite it. Missing/malformed/oversized/encoded/
partial data remains unknown, with no content character billing.

Only active Reject invokes the existing canonical terminal owner with native
event: error and a native error envelope, without Chat DONE. Passive observation
preserves bytes. Client API is captured before AiProxy's own early failures in
existing lazy AI state; ordinary requests allocate no AI state. The supporting
pg_upstream_peer error site delegates to the AI renderer. Generic transport,
physical body-cap and 504 deadline errors retain their HTTP contract.

## Owners and discriminating coverage

Owners: ai/protocols/anthropic, canonical ai/connectors/provider, shared usage and
attempt/quota finalization. Mirrored native units exercise byte splits, ignored
large content, late cache, duplicate replacement/envelope isolation, nested file
binding, malformed attribution, provider-error latching and native Reject framing.
Real Pingora sessions cover live/deferred/late header policies.

EdgionBackend/Messages is separate from the converter mock and uses inventory
port 31990. The canonical integration runner owns static tests, SDK 0.125.0,
A/B/A Secret rotation, raw declaration tightening and ordinary protocol controls.
Final residue validation runs after those checks. Runtime logs and final review
evidence belong in Task 07; mock/SDK success is not live-provider conformance.

Re-evaluate on new certified versions, supported state markers, beta accounting
participants, semantic guards, header seams or response/finalization changes.
