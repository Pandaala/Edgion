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
AI attempt rebuilding restores both live headers and deferred mutations from one
post-rule baseline before each candidate's modifiers and credential isolation.
Skipped candidates cannot delete the deferred version/beta values of a successor;
sensitive HeaderValue ownership, expiry and final Connection checks remain shared.
RequestId reserves Anthropic protocol/identity targets; native RemoteIp collisions
are rejected through the existing session interface. Deferred version mutations
are resolved before observation admission. Only version 2023-06-01 is certified;
optional unsupported-version observation remains sticky unknown and required
accounting rejects before provider dispatch.

Deferred version observation borrows the winning HeaderValue from the canonical
mutation list, whose upsert replaces equivalent normalized names. It compares raw
bytes without constructing or applying a scratch request. This is observation
only: values, sensitivity and deadlines remain owned by the list, and actual
materialization plus expiry/Connection checks retain their existing boundaries.
Live duplicate-version handling and missing-version defaults are unchanged.

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
Every nonempty iteration item requires its own valid final model attribution;
an earlier item's model cannot fill a missing field. SSE carries the first valid
message_start actual model into later delta comparisons, rather than comparing
against a request alias. Same-document duplicate models retain last-wins semantics,
while malformed or duplicate starts cannot repair sticky unknown observation.

Observation off retains NotObserved and unknown model completion. A dispatched
2xx exchange with real EOS can still restore credential transport health through
the shared attempt settlement gate, subject to failure, termination and probe
generation checks. It adds no response parser, numeric usage or model-success claim.

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

## Native request UTF-8 admission (ARCH-012)

Chat, Responses and Messages share the allocation-free borrowed
`ai/protocols/request.rs::native_json_utf8` admission owner before selective
metadata parsing. Ignoring provider strings in the locked serde_json visitor
does not validate their UTF-8. Messages previously omitted the complete scan,
so observation Off could forward malformed opaque strings unchanged, while
optional character counting under Provider/CostWeighted classified them as
BackendUnavailable. Optional accounting and model rewrites cannot own ingress
validity. Invalid bytes anywhere now produce MalformedBody and the fixed native
400 envelope before request-plan publication.

The helper returns the original borrowed string; each protocol retains its
root/syntax, selected-key duplication, field-type and bounded capability checks.
The change adds one linear scan per Messages metadata/rewrite parse of the
physically bounded canonical body, with no body copy, AST, cache or new state.
Existing Chat/Responses full-slice scans are replaced rather than duplicated.

Private protocol tests cover invalid ignored top-level/nested/message strings,
unchanged/aliased rewrites, valid multibyte opaque fields, borrowed model spans
and the existing 256 KiB preservation case. Shared helper tests assert original
pointer/length and zero allocations. AiProxy request-stage tests cross
Off/Provider observation, Weighted/CostWeighted selection and unchanged/aliased
models with fixed native 400, no plan/context publication, no dispatched marker
and an empty attempt ledger.
Canonical sliced-Bytes tests assert retained pointer/length. These are local
parser/request-stage assertions; they do not claim fresh real-wire or live-provider
acceptance. Re-evaluate when native parsers, serde ignored-value handling,
canonical body ownership or predispatch ordering changes.

Local verification on 2026-10-12 passed 66 protocol tests and 21 proxy tests,
workspace check/clippy, formatting, agent-doc consistency, unit-test layout and
the recorded static guards. Independent AIMPL result review passed the final
implementation, including the resolved-Secret request fixture, at scoped content
fingerprint `e40ec63a23a0ceaceca09ac7ef76fd44e2e4d80323df63d1a4ab5484cfdb6c82`.
These results establish the local acceptance above, without live-provider evidence.

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
