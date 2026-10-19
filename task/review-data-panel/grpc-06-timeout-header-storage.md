# Evaluate gRPC timeout header storage

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `bf998947d`.

## Evidence

`pg_upstream_request_filter::upstream_request_filter` formats the remaining gRPC
budget into a String, inserts `&value` into the request header and drops the
String. The locked Pingora `RequestHeader::insert_header` delegates to
`TryInto<HeaderValue>`. In http 1.5.0, the borrowed String conversion validates
and copies bytes; the owned String conversion transfers the buffer through Bytes.

A direct move preserves the header bytes and callback order, but is not an
unconditional allocation improvement. In bytes 1.12.1, converting a Vec whose
capacity differs from its length allocates shared ownership metadata and retains
the spare capacity. The existing borrowed path releases the formatting buffer.

A temporary standalone Rust probe used the actual source formatter via a path
module, pinned http 1.5.0 and bytes 1.12.1, and a System allocator wrapper counting
allocation/reallocation calls and requested sizes. Formatting occurred outside
each measured conversion region. Both HeaderValues remained live for equality
checks, and every owned conversion retained the original payload pointer.
Fresh results on this host:

| Input nanoseconds | Text length | String capacity | Borrowed conversion calls / bytes | Owned conversion calls / bytes |
|---|---:|---:|---:|---:|
| 0 | 2 | 2 | 1 / 2 | 0 / 0 |
| 1 | 2 | 8 | 1 / 2 | 1 / 24 |
| 10 | 3 | 8 | 1 / 3 | 1 / 24 |
| 9,999,999 | 8 | 8 | 1 / 8 | 0 / 0 |
| 99,999,999 | 9 | 16 | 1 / 9 | 1 / 24 |
| 100,000,000 | 7 | 8 | 1 / 7 | 1 / 24 |
| 100,000,000,000 | 7 | 8 | 1 / 7 | 1 / 24 |
| 100,000,000,000,000 | 7 | 8 | 1 / 7 | 1 / 24 |

These are conversion-only requested allocations, not total pipeline allocation,
allocator usable size, retained-memory totals or throughput. Capacities and
metadata layout are implementation/platform observations. Reproduce by formatting
each Duration, recording length/capacity, and separately measuring
`HeaderValue::try_from(&value)` and `HeaderValue::try_from(value)` with freshly
formatted equal inputs; compare exact bytes and original payload pointers.

## Decision and acceptance

Decide whether to keep copying, move the formatted buffer, or design a bounded
stack/direct header formatter. A broader formatter change affects consumers
outside this callback and needs a separately reviewed scope. Do not optimize only
the zero-budget branch or add capacity-dependent branches without evaluating
complexity and actual workload benefit.

Measure formatter plus conversion allocations and CPU, retained buffer capacity
and metadata, and the later transcoder timeout snapshot clone. Preserve the
8-digit grammar, all six units, upward rounding, saturation, zero-budget `1n`,
remaining-budget sampling and request-header writer ordering. Keep the saved
transcoder timeout comparison after later header writers; it detects mutation.
No new per-request instrumentation, tracing or metrics is needed.

Original deadline boundaries are documented in
`skills/04-review/h2-grpc/regression-guards/timeout-honored.md`.
Fresh baseline verification passed 15 upstream-request-filter and 15 timeout
tests, formatting check, workspace check/Clippy and agent-doc validation. These
are unchanged-code checks, not acceptance of a storage redesign. No live wire
suite or throughput benchmark ran. Runtime code remains unchanged.
