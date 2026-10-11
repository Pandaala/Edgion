---
id: ARCH-033
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-319
analyst: /root/analyze_conversion
independent_verification: VERIFICATION-ARCH-033.json
---

# Allocate transcoder callback output only when a streaming message produces it

## Current ownership and concrete cost

`GrpcResponseProcessor::feed` starts with CappedWriter::new(1024 * 1024)
(`response.rs:142`). The canonical Resources writer immediately calls
Vec::with_capacity(cap) (`grpc_transcoding/codec.rs:13`). Unary feed retains
its one bounded protobuf message in self.unary (`:165`, `:177`), while only
the streaming branch writes callback output (`:187`, `:201`). The unused
unary writer is converted back to an empty Vec at `:209`. Unary settle owns
later decoding/replacement after original terminal status, independently of
this empty writer. Incomplete streaming prefixes/messages also produce no
callback output while allocating this capacity.

A unary protobuf response fragmented over several transport chunks requests
a 1 MiB scratch allocation per feed despite zero callback output. Its bounded
message owner and process reservation already exist. The cost comes from
preparing streaming output inside shared framing. This is unnecessary allocator
work and temporary requested capacity, not proof that each allocation commits
1 MiB physical memory or raises RSS by that amount. No benchmark ran.

## Bounded improvement and alternative

Keep private callback output as Option<CappedWriter>, initially absent. Create
the existing fixed-capacity writer when the first completed streaming message
needs JSON output; subsequent complete messages in that callback share it.
Return Vec::new() if no output is produced. Preserve the aggregate 1 MiB
callback cap and canonical writer.

A smaller variant skips writer construction only for unary feed. Lazy creation
also avoids incomplete streaming scratch without another framing engine.
Do not change the shared writer into uncapped growth, retain a scratch writer
across callbacks, or enlarge admission budgets based on these savings.
Only response.rs and relevant tests need change; the Resources constructor
is supporting evidence, not authorization to refactor its other consumers.

## Required invariants and future verification

Preserve the five-byte uncompressed prefix, independent protobuf input bound,
exactly-one-message unary rule, fragmented state and 256 completed-record
callback work bound. Streaming message ownership remains RawWindow retain,
explicit Replace and start_next. Retain independent message JSON, aggregate
callback output and outstanding output-capacity bounds, charging actual Vec
capacity through Reservation until the final Bytes reference drops.

Preserve pre-reflection admission, panic isolation, capped ProtoJSON and
conservative 32 MiB exchange/64 MiB process ledger. Unary decoding and final
headers/body replacement follow immutable original grpc-status validation.
Preserve missing/duplicate/invalid terminal status, header-only errors, sticky
conflicts, deadlines/cancellation and first decisive terminal diagnostics.
Ordinary HTTP/native gRPC/AI/default streaming and incompatible-body-consumer
rejection remain intact. Add no response BodyStorage, spool or full-response window.

Future verification should retain fragmented unary/streaming and coalesced
messages, empty messages, multiple unary frames, framing/cap/credit failures,
real terminal status and deadline/cancellation outcomes. A distinguishing
allocation check must establish that zero-output feeds do not create a writer;
unchanged response assertions alone do not prove that saving.

## Original decisions, deduplication and limits

The original transcoder admission/outcomes memory/time ledger requires byte,
complexity, heap/output bounds and status-before-commit. It permits callback
writer memory at a response peak but does not require zero-output allocation.
Gateway overview/plugin-system records preserve a private explicitly selected
unary owner and no general whole-response API. Response, transport and budget
tests were read for fragmented/coalesced framing, original status evidence,
sticky conflicts, output credit and Bytes lifetimes.

The read-only UpstreamResponseBodyFilter allocation prohibition does not apply
to these explicitly bounded writable conversion processors. No decision/task
requiring an eager writer was found. This is separate from common session
leakage, terminal-header preparation and ARCH-032's outbound request copy.
The constructor/callback paths were inspected statically; no tests/builds,
benchmarks or fresh execution of historical acceptance occurred. Independent
verification is pending.
