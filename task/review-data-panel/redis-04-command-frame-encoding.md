# Redis command frame encoding ownership

Status: deferred dependency design and performance decision for the user.
Source baseline: `c40e2ee1b`; Fred 10.1.0 and redis-protocol 6.0.0 from Cargo.lock.
No runtime or dependency change.

## Current evidence

The shared RedisRateLimiter::eval_script reaches Fred's ordinary EVALSHA/EVAL
command path. In protocol/utils.rs, command_to_resp2_frame and
command_to_resp3_frame allocate an intermediate frame Vec with capacity
args.len() + 2 for ordinary commands. Each argument becomes a frame element.
Integer arguments remain scalar until encoding; String and Bytes arguments clone
byte handles rather than unconditionally copying their payloads. Double and
Boolean values use formatted strings, but this is not evidence that Edgion's
integer script arguments take those branches. Custom commands and RESP3 HELLO
have separate construction paths.

router/utils.rs::write_command prepares the frame before placing the original
EVALSHA/EVAL Command into Connection's in-flight buffer, then awaits the write.
The original command still owns its arguments. Connection::write selects
transport send or feed according to the existing flush decision. Commands with
no responses have a separate push_command path; this observation does not assert
that every command is buffered.

protocol/codec.rs consumes the frame and passes it by reference to the protocol
encoder with integer-as-bulk/blob-string enabled. RESP2 extend_encode computes
the encoded length, extends the destination BytesMut, and writes the frame there.
The intermediate frame array and the encoded destination are distinct storage.
The protocol library also exposes a borrowed-frame encoder, but that API alone
does not remove frame-array construction or solve ownership across the async sink.

Fred's router also owns a retry buffer and checks timed_out before resubmitting
its queued commands. These source observations do not prove all reconnect paths,
exactly-once execution, or cancellation of an already transmitted operation.
Moving arguments out of the original command is therefore not an established
behavior-preserving optimization.

## Decision and acceptance

Keep the existing dependency path, or separately design direct/borrowed command
encoding that avoids the intermediate frame Vec and temporary byte-handle owners.
Potential savings need measurement: no exact allocation count, copied-byte
reduction, latency or throughput benefit has been established. The socket buffer
still needs wire bytes; removing a frame representation does not imply zero-copy
network transmission. A dependency patch or upgrade needs an exact reviewed scope;
do not edit the local Cargo registry or introduce a parallel Edgion Redis client.

For an approved design:

- Compare exact RESP2 and RESP3 wire bytes, including empty/binary inputs, integer
  bounds and negatives, argument ordering, and integer bulk/blob representation.
  Preserve supported value handling and command/subcommand serialization. Keep
  Custom/HELLO on their existing path unless explicitly included and tested.
- Preserve error classification and timing, including rejection before socket
  submission. Failed encoding must not expose a partial command for transmission.
- Preserve original in-flight command ownership, response ordering, flush/feed
  decisions, backpressure, deadlines and cancellation cleanup. Review reconnect
  and retry-buffer behavior independently; do not infer it from successful writes.
- Preserve canonical LinkSys generation, pool routing, health and dial-time egress
  checks. Keep exact NOSCRIPT-only application fallback and existing failure policy;
  do not add replay after timeout, disconnect or ambiguous execution.
- Measure transient frame/container costs separately from retained command and
  socket capacity using network-free fixtures. Any live script-cache experiment
  must use an exclusively owned Redis server. Existing integration fixtures stay
  unchanged; standalone results do not establish Cluster or Sentinel behavior.

This is separate from [fallback input snapshots](redis-02-fallback-command-ownership.md)
and [pool selection](redis-03-pool-member-selection.md). Preserve sensitive-data
redaction and canonical access-log ownership; add no hot-path tracing or metrics
to support this experiment.

## Original knowledge and verification limits

- [Script cache lifecycle](../../skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md)
- [Aggregate pool health](../../skills/04-review/link-sys/regression-guards/redis-pool-aggregate-health.md)
- Source anchors: Fred protocol/{utils,codec,connection}.rs and router/{utils,mod}.rs;
  redis-protocol resp2/encode.rs; Gateway plugins/common/redis_rate_limiter/mod.rs.

This is a source audit, not a new encoder implementation or benchmark. No Rust
tests, live Redis experiment, transport fault injection or reconnect test ran.
