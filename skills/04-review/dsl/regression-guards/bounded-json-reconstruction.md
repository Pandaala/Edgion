---
name: bounded-json-reconstruction
description: Strict raw JSON reconstruction, canonical Body publication and nontruncating Redis GET boundaries.
knowledge_type: regression-guards
---

# Strict JSON reconstruction preserves bounded ownership

## Contract and scope

The generic JSON functions use explicitly encoded strings, not a new DSL value
model or GraphQL registry. Raw values preserve exact numbers and nested variables;
missing nil remains distinct from encoded null. json.set_string performs escaping,
while json.set_raw explicitly validates encoded input. Decoded duplicate keys,
including escape aliases, reject recursively. RawValue alone does not validate
escaped Unicode surrogate pairs: scalar strings also pass a precharged strict
serde string visitor without retaining decoded text. Lone surrogates reject at
top level and inside containers before reconstruction or publication; valid pairs
retain their original encoded spelling. No response-body aggregation exists.

The canonical strict-policy owner is edgion-dsl/src/json.rs. DOM and raw modes
share node/depth and decoded-key duplicate admission. Resources preserves its
existing DOM visitor ordering, numeric interpretation, node/text accounting and
1 MiB/depth64/nodes20000 limits. DSL raw mode is independently 64 KiB/depth32/
nodes4096, lowered by maxStringLen, with syntax-first error precedence. Merely
placing separate parsers in one file would not satisfy policy unification.

ARCH-069: DOM objects use their result map entries as the sole decoded-key
membership owner. Text accounting precedes duplicate admission, and occupied
entries reject before reading the duplicate value, including malformed or
over-budget values. Vacant entries receive only successfully parsed values.
Raw mode retains its separate key set and unchanged preallocation work charges;
both representations still use the same duplicate-error policy. This removes
redundant DOM key copies and indexing, without claiming measured heap or throughput
improvements or changing the raw allocation ledger below.

## Allocation and execution boundaries

Each raw serde invocation charges four units per span byte for traversal plus
four for escaped-token scratch and capacity overlap before parsing. Repeated
nested RawValue scans pay again. Key visitors charge before retaining decoded
strings; duplicate copies/sets, vectors, nodes, key lists and Arc copies are
separate non-refundable charges. Capped output writing charges growth and copied
bytes while appending, including escaping. Work limits can reject pathological
inputs below scalar limits; these are producer bounds, not process-wide memory
admission or guarantees about allocator rounding.

Gateway DslBodyOps owns complete Body length admission, physical/scan accounts and
the existing blocking lane. Malformed JSON/UTF-8 becomes nil; Body resource/storage/
work failures are fixed fail-close. JsonSetBody validates a complete encoded
string before one session overwrite and checks request_info.in_request_body_stage
before allocation. Source/wire capture and mutation analysis recognize it;
response factories reject it. Every publication failure is BodyAccess500 even
under Ignore. Security-inspection freeze and canonical framing remain authoritative.

RedisGetBounded uses `redis::cmd("GET")` on the live LinkSys client. Cluster
routing uses `command_route` with key index 1. Positive bound
admission precedes provider resolution; effective cap is min(requested,maxStringLen,
65536). Borrowed reply type/length/UTF-8 preflight precedes text allocation. Fixed
oversize/invalid-response/timeout classes preserve the non-throwing map. Typed
redis-rs RedisError::is_timeout() is classified before formatting. Raw server errors frames are dependency failures; classify them using a fixed private
error rather than copying an unbounded or sensitive server message. Successful
non-string replies remain invalid_response. Existing pool command
timeouts and caller cancellation apply; the VM has no general wall-clock timeout.
redis-rs's full reply aggregation is outside this conversion bound. Old get truncation
is unchanged and is not safe for complete stored query records.

## Evidence and re-evaluation

Owning tests cover exact numeric/nested values, escaped duplicate names, missing/
null, work/output bounds, DOM competing errors and profile preservation, source/
wire arity and capture, memory/spill reads, one/zero publication, frozen generation,
late-stage rejection, fixed Ignore failure and raw Redis type/byte/UTF-8 admission.
Fresh test/check/review results belong in the selected task's implementation record;
this page does not claim live recipe or response-cache acceptance.

Re-evaluate on serde/redis-rs upgrades, parser policy/materializer changes, allocation
ledger changes, new raw operations, Body publication/stage ownership changes or
Redis transport memory guarantees. The maintained architecture contract is in
[the DSL engine security model](../../../01-architecture/02-gateway/dsl-engine-security-model.md#strict-json-reconstruction-and-nontruncating-redis-get).

The historical source task was
`response-cache-and-dsl-query-lookup`; its task file is absent from the current tree.
