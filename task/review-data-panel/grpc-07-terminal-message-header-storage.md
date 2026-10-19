# Evaluate terminal gRPC message header storage

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `079e3ad37`. No runtime change.

## Evidence

`edgion-gateway/src/runtime/server/error_response.rs::send_grpc_trailers_only_frame`
uses the canonical `percent_encode_grpc_message` for both native gRPC and gRPC-Web.
The native branch passes the owned String into Pingora insertion. The Web branch
calls `.parse()` on that String, which validates and copies it into HeaderValue,
then drops the temporary encoding buffer. The recently optimized decimal status
field has exact capacity on this build; encoded message buffers need not.

Locked http 1.5.0 transfers an owned String through Bytes. In bytes 1.12.1,
a Vec with capacity greater than length retains that capacity and allocates Shared
metadata. Therefore replacing Web message parsing with owned conversion does not
uniformly reduce allocation calls or requested bytes.

A standalone Rust probe extracted the actual canonical ASCII set and encoder
function from the current source and linked the existing locked artifacts. A
System allocator wrapper bracketed encoding plus HeaderValue construction, with
allocation/reallocation call counts and requested sizes. Printing and the separate
length/capacity observation occurred outside counting. Both paths produced equal
HeaderValues with equal unset sensitivity flags:

| Input | Encoded length / capacity | Current Web calls / bytes | Owned calls / bytes |
| --- | --- | --- | --- |
| empty | 0 / 0 | 0 / 0 | 0 / 0 |
| `x` | 1 / 8 | 2 / 9 | 2 / 32 |
| `quota` | 5 / 8 | 2 / 13 | 2 / 32 |
| `unavailable` | 11 / 11 | 2 / 22 | 1 / 11 |
| `quota 50% exceeded` | 20 / 32 | 4 / 76 | 4 / 80 |
| `%` | 3 / 8 | 2 / 11 | 2 / 32 |
| `line` + newline + `break` | 12 / 16 | 3 / 36 | 3 / 48 |
| `café` | 9 / 16 | 3 / 33 | 3 / 48 |
| `π` | 6 / 8 | 2 / 14 | 2 / 32 |
| `abcdefghijklmnop%` | 19 / 32 | 3 / 67 | 3 / 72 |

These are this build's requested allocations, including reallocations, not live
or peak heap usage, allocator usable sizes, CPU measurements or throughput.
Reproduce by measuring `percent_encode_grpc_message(input).parse::<HeaderValue>()`
and `HeaderValue::try_from(percent_encode_grpc_message(input))` separately, keeping
values alive for byte and sensitivity comparisons after counting stops. Do not
compare these complete-construction totals directly with the conversion-only
measurements in [grpc-06](grpc-06-timeout-header-storage.md).

## Decision and acceptance

Decide whether to retain the current copy, accept direct ownership transfer's
workload-dependent tradeoff, or evaluate exact-capacity/borrow-aware construction
through the canonical encoder. An extra sizing pass can reduce growth and retained
capacity while adding work; it requires CPU and representative-message evidence.
Do not add a second percent encoder or capacity-dependent fast path without
reviewing the complexity and whole-path benefit. Changing the shared encoder
requires reviewing both native and Web consumers, not just this Web call site.

A future IMPL needs an exact allow-list and independent reviews. Preserve literal
spaces, percent escaping, C0 controls, DEL, UTF-8 byte encoding, absent versus
present-empty messages, unset sensitivity and exact bytes. Preserve canonical
status/message ownership over queued fields, allowed unrelated headers, H1 bridge
trailer output, native H2 HEADERS+END_STREAM, final-response claims, delivery-error
propagation and cancellation. Do not alter deadlines or broaden response buffering.

Compare common mapped messages, dynamic DSL/ExtProc messages, empty/short/long
inputs, dense escaping, and mixed UTF-8. Measure encoding plus conversion, copy
cost, allocation calls/bytes and retained capacity separately. No request-path
instrumentation is needed. The canonical encoding contract remains in
`skills/04-review/h2-grpc/regression-guards/grpc-message-encoding.md`.

Fresh unchanged-code baseline verification passed 15 server error-response,
16 fail-to-proxy and 178 session-adapter tests (209 total), formatting check,
workspace check/Clippy and agent-doc validation. Existing unrelated warnings
remain. This is not acceptance of an alternative or a live integration result.
