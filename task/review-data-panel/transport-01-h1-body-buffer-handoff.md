# HTTP/1 body-buffer handoff ownership

Status: deferred for user decision. No runtime or dependency change.
Edgion baseline: `f3f5416b8`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and reachability

The locked fork's pingora-core/src/protocols/http/v1/body.rs owns a reusable
BytesMut reader buffer and returns BufRef ranges. prepare_buf_for_io clears and
reserves that buffer only after its buffered bytes have been consumed. Partial
chunk headers, pending rewind and trailer tails have separate preservation paths.
Existing init_*_owned/prepare_owned_buf already adopt owned preread storage;
that optimization does not transfer each parsed output chunk to its consumer.

Two owned-output boundaries copy payload bytes:

- server.rs::read_body_bytes_inner calls Bytes::copy_from_slice on the reader's
  range. It then accounts received bytes, updates optional retry retention and
  awaits optional early body capture, preserving cancellation poison and finish.
- client.rs::read_body_ref updates received-byte accounting while exposing a
  reader slice; read_body_bytes copies that slice into Bytes. read_response_task
  returns Body or UpgradedBody tasks, which the proxy pump subsequently queues.

proxy_h1.rs::send_body_to_pipe runs the request-body callback path before moving
owned data into the selected task. send_body_to1 borrows that owned Bytes while
awaiting the writer. The slice cannot simply outlive reader reuse through these
filters, queues and asynchronous writes. A copy is an ownership boundary here,
not evidence of a redundant clone that can safely become a reference.

Successful H1 upgrades use close-delimited body readers under the existing
request/status/method gates; 101 alone is not a universal initialization rule.
WebSocket tunnel payload therefore reaches these H1 ownership boundaries too.
These routines transport bytes rather than decode/re-encode WebSocket frames.
The protocol discovery label does not bypass body hooks. No H2 Extended CONNECT,
handshake validation, Origin policy or forced-upstream-H1 policy change is implied.

## Decision and scope

Retain the independent compact copy, or design a narrowly scoped owned-chunk
output from the canonical H1 reader, for example splitting/freezing consumed
storage while preserving unread tails. Removing a copy can require replenishing
reader capacity or retaining shared backing allocations until queued outputs are
dropped. Header preread, chunk framing and partial trailers may share that storage.
Do not infer lower allocation count, peak memory or CPU from fewer copied bytes.

Measure both directions and ordinary versus upgraded traffic before selecting a
representation. This is distinct from retaining empty compressor output buffers
and from bounded plugin-initiated response aggregation: neither owns these reader
buffers. Do not add a Gateway-local H1 decoder, special raw tunnel bypass,
response BodyStorage, general aggregation, global pool or speculative unsafe view.

Any implementation requires explicit user selection and an exact external
Pingora checkout/file allow-list, independent plan/final review and separately
reviewed dependency adoption. Do not edit Cargo's cached checkout or update its
revision as incidental cleanup. This task authorizes neither operation.

## Acceptance before implementation

- Preserve content-length, chunked and close-delimited framing, partial headers,
  trailers, preread/overread and pipelined-next-request bytes. Preserve successful
  and rejected upgrade behavior, including protocol-specific termination gates.
- Keep already returned chunks immutable and valid across subsequent reads,
  queued writers, slow delivery, callback mutation, cancellation and shutdown.
  Include readers with retained tails and small output slices of large backing
  buffers; measure live capacity and replenishment separately from copy counts.
- Keep request capture/replay/retry and poison-on-cancellation semantics, payload
  counters, body limits and upgraded exemptions, deadlines and error ownership.
  Preserve channel capacity reservation/backpressure and all callback ordering.
  Do not collapse Some(empty) and None, suppress final observations, duplicate
  EOF/footer/trailer delivery or turn a failed capture into a partial replay.
- Verify actual fork reader and proxy-pump behavior with in-memory fragmented
  input, retained independent outputs and existing transport regressions. Exercise
  Gateway body filters, cancellation and a live upgrade separately before making
  an end-to-end claim. No bypass of canonical access logs or new hot-path tracing.
- Benchmark allocations, copied bytes, CPU and simultaneous memory under multiple
  chunk sizes, consumer speeds and concurrent connections. Report both direction
  and framing mode; this proposal does not establish a zero-copy whole proxy.

## Knowledge and verification limits

Start at skills/SKILL.md, then websocket intentional-design knowledge, the
http-body ownership/capture rules, and the h2-grpc trailer-done terminal-body
regression guard. Revalidate those rules against the selected fork revision.

Fresh unchanged-code Gateway runs passed 19 request-body-action and 31 upstream
response-body-filter tests. Formatting, workspace all-target check/Clippy and
agent-doc validation passed with existing warnings. These are current Gateway
regressions, not a fork ownership prototype, fresh fork suite or live WebSocket
probe. No runtime test was added and no performance benchmark ran.
