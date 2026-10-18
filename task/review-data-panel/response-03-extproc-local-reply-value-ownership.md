# ExtProc local-reply value ownership

Status: deferred optimization for user decision. No runtime change.
Source baseline: 836e273db.

## Evidence

`plugins/http/ext_proc/mutation.rs::local_reply_ops` consumes HeaderMutation and
already moves accepted names. Its shared decoded_value helper borrows raw_value
(or the textual fallback), validates it with HeaderValue::from_bytes and copies
only the final value bytes. The decoded source buffer is released after conversion.
Request-stage immediate_reply queues these typed operations through the session
adapter into the context. The streamed request-body companion carries them in
ChunkVerdict::Terminate; write_terminate_reply applies them to the local response.
PendingHeaderOp::apply moves the typed value into the response header.

An owned HeaderValue conversion could avoid the value copy. In locked http 1.5.0,
TryFrom<Vec<u8>> and TryFrom<String> use shared Bytes storage. Locked bytes 1.12.1
retains Vec capacity and allocates a Shared owner when length differs from capacity.
Locked prost 0.13.5's Vec BytesAdapter clears and reserves on replacement, so a
repeated singular raw_value field can replace a long value with a short value
without releasing the earlier capacity.

A synthetic dependency probe using the same three HeaderValue field tags and
locked versions encoded raw_value (tag 3) twice: 65,536 ASCII bytes followed by one
ASCII byte. Decoding produced length 1 and capacity 65,536. Borrowed conversion
made a separate value; owned conversion preserved the source pointer and equal
final bytes. This is a reproducible dependency-level witness, not a gateway wire
integration test, throughput measurement or assertion of unbounded memory.
The probe does not establish the precise downstream release time under load.

## Decision

Unconditional ownership transfer can retain decoder spare capacity through
pending-operation and response-header ownership instead of releasing it at
conversion. The allocation/copy saving is therefore not established as impact-free.
Choose whether to retain copying or investigate a bounded transfer policy, such
as moving only exact-capacity values and copying other values. Unconditional
transfer would need explicit acceptance of measured retained-memory costs.
A selective fast path is not rejected; its benefit and validation behavior need
a separate bounded IMPL plan. Shrinking first may reallocate and is not a free
substitute for that analysis.

This concerns inbound local-reply header values only. It does not authorize
changes to borrowed request/response mutation APIs, trailer candidates, protobuf
code generation or the separate outbound HttpBody representation decision in
[body-02](body-02-extproc-protobuf-body-storage.md).

## Acceptance for a selected implementation

- Preserve nonempty raw_value precedence, text fallback, keep_empty_value,
  invalid-byte rejection with static logs, protected-name rejection, all four
  append actions, set-before-remove order and local-reply condition timing.
- Keep one canonical validation policy; do not fork borrowed and owned decoding
  semantics or introduce unsafe unchecked HeaderValue construction.
- Test exact/spare-capacity raw and text values, empty and opaque values, invalid
  bytes, and duplicate singular protobuf fields with long-to-short replacement.
  Compare value bytes and existing sensitivity behavior explicitly.
- Measure copying/allocation and retained capacity separately, including dropped
  operations, replacement, delayed response writes and cancellation. Do not infer
  whole-request lifetime or throughput from an ownership pointer alone.
- Preserve request-stage and streamed immediate replies, fail policies and
  terminal ownership. Run network-free focused tests and required repository
  checks; any wire tests use the existing shared integration topology.

Relevant canon: ExtProc opaque-header-fidelity and interactive-continue-and-replace
regression guards, PluginSession typed writes, and PendingHeaderOp ownership.
Freeze the implementation allow-list in the new plan after the user selects scope.
