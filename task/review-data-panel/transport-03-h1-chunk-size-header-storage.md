# HTTP/1 chunk-size header storage

Status: deferred for user decision. No runtime change.
Source baseline: Edgion `763f98120`, pinned Pingora `1aaa45c`.

## Evidence and scope

The canonical fork BodyWriter formats each chunk-size line with
`format!("{:X}\r\n", size)` and converts the String into Bytes. Both
`do_write_chunked_body` and `poll_write_chunked_body_task` use this heap-backed
header. The existing chained buffer already keeps header, payload and trailing
CRLF separate: this is not a proposal to eliminate a concatenated payload copy.

The legacy writer borrows payload through its awaited write. The poll writer
moves payload into its owned chained WriteBuf and preserves progress across
Pending. The shared BodyWriter serves H1 server response writes and H1 client
request writes, so an implementation must check both directions.

The H1 and H2 upstream response pumps call `write_response_tasks`, which runs
modules before downstream `response_duplex_vec`. H1 downstream dispatch handles
one task directly but buffers body data for multi-task batches before calling
write_body_buf. Thus this header cost follows actual chunked writer invocations,
not every Gateway callback or original upstream chunk. Ordinary downstream
adapters suppress empty bodies; BodyWriter itself does not universally suppress
an empty input. The separate poll API defaults off in H1 sessions, and no Gateway
setter was found; its state semantics still matter for a shared-fork change.

Content-length and close-delimited modes do not use this size formatter. Known-
length response compression can remove Content-Length before H1 framing, while
unknown-length responses remain ineligible for Gateway compression. This proposal
does not change compression eligibility, batching, flushing or HTTP framing.

## Decision alternatives

- Keep the current small heap header unless measured frequency and cost justify
  another buffer representation.
- Use a bounded inline hexadecimal header in the canonical writer. At most
  `usize::BITS / 4 + 2` bytes hold uppercase digits and CRLF. Legacy storage must
  live through the awaited write; poll storage and its cursor must be owned across
  Pending/cancellation/resume, with no self-referential borrow. Compare one shared
  representation with a legacy-only change rather than assuming both need redesign.

An inline header can remove this String owner but enlarge a future or writer
state and alter buffer dispatch. No prototype, allocation count, CPU benchmark or
net throughput/RSS improvement has been established. This differs from
[H1 reader-buffer handoff](transport-01-h1-body-buffer-handoff.md) and compressor
output retention; neither owns the chunk-size line.

Implementation requires user selection, an exact external Pingora checkout/file
allow-list, IMPL plan/final reviews and separately reviewed dependency adoption.
Do not edit Cargo's cached checkout or update its revision as incidental cleanup.
This task grants neither permission.

## Acceptance before implementation

Preserve uppercase hexadecimal with no leading zeros, exact payload/CRLF, empty
input versus None, end-of-body and trailers. Verify zero, digit-width boundaries
and usize maximum formatting without allocating a maximum-sized payload. Preserve
content-length, close-delimited and upgraded paths, downstream module ordering,
batching, backpressure and deadlines.

Exercise partial writes at header/payload/trailing-CRLF boundaries, vectored and
non-vectored transports, Pending before/after each boundary, write/flush errors,
timeout and poll cancellation/resume. Preserve existing accounting timing:
legacy body-mode accounting advances after flush, poll accounting before flush;
outer session counters have their own successful-return boundary. Do not combine
this optimization with a change to those contracts or to flush frequency.

Measure allocations, CPU, future/state size and live memory for small/high-count
chunks and larger bodies in both directions. Run actual fork writer regressions
and Gateway callback tests; Gateway-only green tests do not validate a changed
fork writer. Keep canonical access-log ownership and add no hot-path logging,
unsafe borrowed view, global pool or alternate Gateway framing implementation.

## Sources and baseline verification

Pinned fork sources: `pingora-core/src/protocols/http/v1/body.rs` (BodyWriter,
WriteBuf), `v1/server.rs` (response_duplex_vec, write_non_empty_body,
write_body_buf), `v1/client.rs` (request writer), and
`pingora-proxy/src/{lib,proxy_h1,proxy_h2}.rs` (response-task dispatch).
Original knowledge: `skills/04-review/http1/compatibility-constraints/compression-requires-known-response-length.md`.

Fresh unchanged Gateway tests passed 82 cases: 31 upstream response body, 19
request body action, six response callback and 26 final response filter tests.
Formatting and workspace all-target check/Clippy passed with existing warnings.
No fork tests, live transport, new tests, runtime/dependency/fixture changes or
benchmark ran. Source inspection establishes the allocation site and reachability,
not full dependency correctness or optimization acceptance.
