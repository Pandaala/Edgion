---
name: grpc-json-transcoder-admission-and-outcomes
description: Review unary owner compatibility, terminal diagnostics, ProtoJSON admission and mapping disjointness.
knowledge_type: regression-guards
---

# GrpcJsonTranscoder admission and terminal outcomes

## Conclusion and scope

The unary response replacement bypasses upstream body processors. Its owner therefore
rejects ExtProc effective response-body consumers, installed ordinary processors,
BandwidthLimit and response-body Wasm, across actual global/Gateway/rule/selected
backendRef targets. Header-only ExtensionRefs must not be rejected merely because
an outer wrapper implements the body trait. WAF's intentional empty phase-four window
is unchanged. This correction introduces no full-body replay or new response cache.

## Typed transport classification (ARCH-005)

Resources owns the exhaustive typed AI conversion-conflict category through
`EdgionPlugin::is_ai_conversion_owner`; both resource validation and Gateway
successful request construction use it. The Gateway response-body factory returns
an explicit unary conflict fact alongside each successful filter, including the
separate Wasm body construction. Existing EntryBundle cache publication transfers
both facts into effective immutable runtime views. Admission getters read those
facts rather than scanning trait names.

Executable membership is authoritative: disabled, Block/Bypass, wrong-stage and
failed constructions do not publish phantom members. Conditions remain conservative.
Ready/Failed alone is not a membership proof; a failed bundle may carry successful
executables alongside an independent failure. Wasm runtime construction can succeed
while its best-effort preload or later module load fails; that executable still
publishes its stage-specific conflict fact under the existing failure policy. Policy views do not acquire flags
from cached slots they omit. Released source slots do not erase flags or filters
from captured runtime Arcs, and replacement generations construct their own facts.

Header/request Wasm, header-only DSL and outer ExtensionRef wrappers remain
compatible. The selected-backend check still resolves the exact request-pinned
target. ExtProc effective-mode and installed-processor checks stay dynamic, and
sticky conflicts, installation ordering and WAF's empty phase-four window are
unchanged. This is a bounded ownership refactor, without a throughput claim or
new response buffering/dispatch mechanism. Re-evaluate when adding AI owners,
response-body factory outcomes or changing effective-view publication.

## Rationale and evidence

Task 09-grpc-json-transcoding completed its second Astra review against baseline
4dbcc5495 plus the working-tree corrections on 2026-10-07. Canonical context registration and the
session adapter reject both installation orders. ExtProc reports effective mode under
its existing lock; mode overrides cannot expand NONE into STREAMED. Rejected late
registration leaves a sticky conflict even under fail-open. Response head planning,
processing, settlement and replacement consumption recheck that conflict. Selected
backend admission resolves the same request-pinned ExtensionRef used by execution,
before request plugins/origin, with AI registration and outgoing-attempt defenses.

GrpcTranscodingPlan retains a fixed terminal outcome independently of output cleanup.
The first decisive failure survives cancellation; universal logging enriches the
original plugin entry once and publishes validated status through RequestInfo. There
are no per-chunk logs, payload/grpc-message diagnostics, new metrics or request tracing.

Request capture admission also belongs to the transcoder: a generic body-gate
wrapper can reject before its request callback and emit an empty text/plain 500,
violating the fixed REST error dialect. The runtime passes the resolved body
requirement into its constructor instead. Capture failures reuse the shared
body-read classification and diagnostics, then render the fixed JSON error.
The operator's stricter request limit is compiled once and enforced before
materialization, independently of the 1 MiB conversion cap and physical cap.

Resources normalizes bounded JSON before reflection: validate names and aliases first;
remove ordinary null fields, including whole repeated/map fields; preserve singular
and element Value nulls. Count the protobuf/WKT shape, including Struct entries,
ListValue/Value expansion and FieldMask paths, before typed/reflected allocation.
Each ordinary message drops its temporary field-number uniqueness set after the
complete name/alias validation pass, before null pruning and recursive traversal.
The set contains owned integers, and later descriptor lookup does not borrow it.
This shortens parent/child temporary-storage overlap without changing validation
order, node charging, reservation or allocation count. It establishes no measured
RSS or throughput improvement; existing behavioral tests do not instrument the
set's production deallocation point.
Only the nine exact canonical scalar-wrapper names take the wrapper shortcut;
similarly named ordinary messages in google.protobuf follow full field traversal.
The [reviewed memory and time ledger](#memory-and-time-ledger) includes temporary
typed trees, capacities and transcode buffers under existing limits.

Integers use exact bounded decimal/exponent scanning for strings and exclusive upper
bounds for floating values. Finite numeric float strings cannot become infinity.
A borrowed recursive serde adapter renders nonfinite wrappers as canonical strings,
forwards collect_str to the capped writer, and preserves Value's nonfinite rejection.
Binding insertion accepts either parent alias but rejects alias duplication and leaf
collisions. Publication proves same-verb path languages disjoint with one bounded
All-match automaton per verb, including EOI transitions and one aggregate work budget.
Exhaustion fails validation; runtime ambiguity defense remains.

## Conversion bounds and source ownership

The explicitly authorized unary exception remains governed by
[the HTTP body rules](../../../../AGENTS.md#http-body-handling-invariants).
It receives exactly one uncompressed gRPC response frame and validates original
terminal status before committing final downstream headers/body. It adds no
response BodyStorage, disk spool, full-response RawWindow or general whole-body API.

Independent hard maxima are 1 MiB request protobuf/JSON, 512 KiB response protobuf,
1 MiB serialized JSON including stream envelopes, depth 32 and 16,384 admitted
protobuf-shape nodes. Query/path input is capped at 64 KiB and bindings at 64.
Response header templates are capped at 8 KiB/64 fields. Callback output and total
outstanding output capacity are independently capped at 1 MiB. Configuration may
select stricter supported limits. Byte caps do not substitute for structural or
heap admission. Compiled descriptor input has its separate 512 KiB cold-path cap.

Canonical compiler/codec ownership is in `edgion-resources/src/grpc_transcoding/`;
exchange admission and output credits are in
`edgion-gateway/src/plugins/http/grpc_json_transcoder/budget.rs`, framing/conversion
in `response.rs`, and plan/header/deadline ownership in `transport.rs`.
The [operator guide](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/grpc-json-transcoder.md)
describes configuration and scope. H1.1/H2 downstream selection is independent of
mandatory H2 upstream; canonical backend/TLS/egress and pool owners are preserved.
Unary Hold replacement currently abandons the origin connection.

### Unary terminal-error cleanup boundary

A direct call to the transcoder's trailer callback can leave unary Vec capacity
retained after settlement fails: one branch clears its length, while decode errors
return before the successful release. Production callers use
ResponseProcessorDriver::finalize_grpc_transcoding under BodyExecutionLease.
Error propagation drops the incomplete lease synchronously, invoking
BodyProcessorSet::release_inflight and the transcoder's release method, which
replaces unary storage with an empty Vec. No await separates settlement error
propagation from this cleanup. The other internal settle call is streaming-only.

Direct callback tests bypass this lease and do not establish production retention
beyond the error boundary. Keep the existing cleanup owner; no additional local
buffer transfer is selected on this evidence. Re-evaluate new settlement callers,
lease completion/error propagation changes, or changes to release_inflight. Existing
transcoder outcome tests and response driver cancellation tests support their
respective boundaries; source inspection establishes their production connection.

### Gateway request input handle release

After synchronous encoding (including a caught error or unwind), the request
entry explicitly drops its local input Bytes before result handling and body
publication. Encoding returns an owned binding/frame and borrows no input bytes.
The conversion reservation and canonical BodyStorage retain their own owners.
Initial memory and cached initial-disk materializations may remain shared with
storage; an independently materialized mutation snapshot can be released earlier.
This shortens local ownership, not the lifetime of every source allocation or a
measured RSS bound. The conservative exchange reservation is unchanged.

### Current query binding ownership

The request entry borrows request_query directly into synchronous binding.
The adapter reads the same current URI as get_query, without its intermediate
String copy. BoundRequest owns its selected binding and decoded fields before
body capture begins; no query borrow crosses that await. Path/query limits,
duplicate-field rejection, percent decoding and REST error routing are unchanged.
The runtime regression covers absent/empty queries, an encoded scalar, duplicate
scalar rejection and malformed escapes, including rejection before capture.
Resources codec tests retain decoded-value coverage. This removes one temporary
query copy, not all binding allocations or a measured throughput bottleneck.

## Callback output acquisition (ARCH-033)

`GrpcResponseProcessor::feed` owns an optional callback-local canonical CappedWriter.
The first completed streaming message acquires its 1 MiB capacity after successful
ProtoJSON conversion and RawWindow Replace; subsequent messages in that callback
share it. Empty feeds, fragmented unary input and incomplete streaming messages
return an empty Vec without constructing the callback writer. Unary retention and
status-before-publication remain independent. Error paths drop callback scratch;
no writer is retained between callbacks and no response cache is added.

The former eager constructor requested unused 1 MiB scratch on every zero-output
feed. This change removes that request at the framing owner, without changing the
Resources writer or claiming measured allocator latency, RSS or throughput gains.
Per-message JSON, aggregate callback output, 256-record callback work and actual
outstanding Vec-capacity credits remain independent bounds; the reviewed exchange
and process reservations below are unchanged.

The mirrored `response_test.rs` constructor observer uses a thread-local counter
only in test builds. `callback_writer_follows_completed_stream_output` distinguishes
eager construction from lazy acquisition and covers empty, fragmented, completed
and coalesced NDJSON/SSE messages, including empty protobuf messages and later
callbacks. `callback_writer_preserves_completed_record_work_bound` checks 256/257
records. `callback_writer_preserves_aggregate_and_message_json_caps` checks valid
individual messages whose coalesced JSON exceeds the callback cap, using escaped
string output while keeping raw coalesced wire below 512 KiB, and independent
per-message overflow. Returned empty Vec capacity alone is insufficient evidence
because the canonical writer compacts on return. On 2026-10-12 the distinguishing
regression failed with eager acquisition through the same constructor seam (10
constructions versus zero), then passed with lazy acquisition. All 25 focused
Gateway transcoder tests passed, including the 11 retained response identities
and these three added tests. All 25 focused Resources grpc_transcoding tests
also passed. These checks do not measure allocator latency or
rerun the full suite/live topology. Re-evaluate when changing the constructor
seam, streaming output owner, cap or callback work bound.

## Memory and time ledger

Use one process-wide 64 MiB conversion budget, shared across plugin instances and
hot-reload generations. Initially reserve 32 MiB per admitted exchange before any
body materialization/reflection allocations. This admits two concurrent conversions;
configuration does not silently increase this limit. Release on every failure,
success and cancellation; reservation ownership follows output Bytes through
Bytes::from_owner until the last writer/reference drops. Compiled generations are
cold-path bounded by descriptor/compiler counts and are separate from exchange
admission. Freeze method descriptors by shared Arc rather than copying per request.

The 32 MiB reservation uses the following conservative peak ledger (the request
and response peaks are alternatives, not concurrent full trees):

- 16 MiB structural allowance: at most 16384 admitted protobuf-shape nodes at
  1024 bytes each. This covers the bounded JSON tree, reflected message/field and
  map-entry nodes, temporary prost_types Struct/ListValue/Value trees, BTreeMap
  occupancy, and Vec spare capacities. The JSON tree is explicitly dropped before
  reflection. Schema admission counts every WKT container, Value oneof, map entry
  and key, repeated element, and nonempty FieldMask path before dependency visitors
  allocate those shapes. This is deliberately more conservative than JSON-node count.
- 14 MiB variable-buffer allowance at the request peak: original canonical request
  and materialization/scratch, normalized serialized input (each <=1 MiB), WKT
  FieldMask text (camel-to-snake expansion <=2 times bounded input; growing String
  capacities <=4 MiB in aggregate), reflected text, and temporary transcode_from
  encoded bytes (text plus <=16 bytes per admitted node, below 2.25 MiB).
  Temporary WKT trees and transcode buffers are released before final framing.
  After reflection and the JSON end check succeed, the deserializer (including its
  scratch) and normalized serialized input are explicitly dropped before encoded
  length calculation and frame allocation. DynamicMessage owns its field data.
  Materialized body and reflected message can remain live during framing; the same
  conservative 14 MiB allowance includes that overlap and a frame <=1 MiB plus five
  bytes. Reservations are unchanged; earlier buffer release is not a measured RSS
  reduction. Output encoding rejects oversize payloads before frame allocation.
- The response buffer peak fits inside the same 14 MiB: canonical request replay,
  RawWindow backing/resident storage <=2 MiB, coalesced wire <=512 KiB, reflected
  and temporary transcode_to typed/text/wire representations bounded by that wire
  plus the admitted nodes, capped JSON writer and callback batch <=1 MiB each,
  and <=1 MiB outstanding output credits. The borrowed recursive serializer adds
  no JSON tree and forwards collect_str to the capped streaming formatter.
  After successful JSON serialization, the reflected message is explicitly dropped
  before envelope suffix writing and output-buffer compaction. The writer owns all
  emitted bytes; no field borrow survives serialization. This reduces local tree
  lifetime overlap without changing caps, reservation or error ordering, and makes
  no measured RSS or throughput claim. WKT transcode temporaries during serialization
  remain a separate optimization decision.
- The remaining 2 MiB covers bounded binding selectors, JSON parsing scratch,
  metadata (header template <=8 KiB/64 fields), driver state, and allocator slack.
  String-producing path/query bindings transfer already-owned decoded buffers
  rather than copy their text; borrowed inputs still become owned. Include retained
  decoder spare capacity and temporary decoding overlap in this allowance, not
  just decoded lengths. The public combined encoded path/query cap is 65,536 bytes;
  path captures are disjoint and query components occupy nonoverlapping segments.
  Path decoding reserves encoded segment length, while query percent decoding may
  retain Vec growth capacity from push/extend. No equal-capacity or exact-size
  assumption is made. Re-evaluate this ownership allowance if input caps, decoder
  growth behavior, capture overlap or retained binding lifetimes change.

Structural limits alone are not a text/heap bound: fixed input/text/wire limits
remain independent. Request schema admission and response wire admission both run
before reflection. FieldMask expansions and temporary typed WKT representations
are explicitly included rather than relying on a post-allocation encoded-size check.
Revisit this ledger with any dependency container/layout change, new WKT, cap increase
or new retained representation; do not silently increase the 32 MiB reservation.

Use the existing canonical remaining request/backend deadline. Unary Hold has a
real absolute timer with a 30 s maximum. Streaming caps each upstream read/write
timeout at 30 s (and the remaining route/backend budget at dial), and checks a
30 s owner expiration at every processing/terminal callback. This is not a hard
wall-clock stream lifetime: waiting for a transport event or downstream write can
overrun that expiration until the existing I/O bound or next callback fires.
Do not claim a total streaming deadline or add a new fork cancellation seam.
Check deadlines at frame conversion, capped serialization and unary replacement
consumption. Error-terminal credit exhaustion aborts without an uncapped fallback.
No raw grpc-message or descriptor/payload/error strings enter client errors or logs.
Use static PluginLog tags and canonical access-log diagnostics; add no metrics.


## Verification and re-evaluation

Recorded completion evidence from 2026-10-07, against baseline 4dbcc5495 plus its
corrections: five new resource regression groups failed on the original code. The corrected
Resources grpc_transcoding tests and Algo regex_disjoint tests passed during development.
Gateway owner/outcome tests, shared-topology negative cases with upstream-count
assertions, and terminal access-log cases accompany the correction. Independent runtime,
AI/integration, corrected codec and supplemental fixture Astra review scopes passed.
The codec review found an additional custom-message wrapper-classification P1;
a private pre-reflection regression failed before the exact-nine-name fix and passed
afterward. Independent CLI processes used gpt-6-astra because native capacity was exhausted.
Locked workspace check/clippy, 1,058 focused regression tests (zero failed, two existing ignored), eleven guards,
40 live transcoder cases, Responses/native protocol regressions, both hot-reload
scenarios and residue validation passed. The request/response-header-only ExtProc
companion sends request headers because the existing FakeProcessor requires that
phase before responding; both body modes stay NONE and fail-close remains enabled.
The first live run failed those two companion cases because the request phase was
skipped; the fixture correction preserved both body modes and fail-close. Both orders
then passed, and canonical logs recorded phases=2 and successful completion. The tests
do not independently assert header markers. Resource codec had 24 passing tests, Algo
overlap 2 and Gateway transcoder 22; AI, ExtProc, session/context, WAF, Controller and
resource schema/type regressions supplied broader coverage. Mirrored regressions and
shared-topology cases remain in their owning crates, including
`edgion-resources/tests/unit/grpc_transcoding/normalize_test.rs` and
`edgion-tests/src/client/suites/edgion_plugins/grpc_json_transcoder.rs`.

The shared topology `tmp/integration_testing/testing_20261007_151608` passed
40 transcoder, nine Responses, three native gRPC and six ordinary HTTP cases, plus
GrpcJsonTranscoderDynamic and AiResponsesContinuation. Cache residue validation wiped
111 resources with zero errors. Standard teardown stopped owned services; Gateway
required the existing five-second SIGKILL escalation, so this is no graceful-shutdown
claim. These are historical results, not execution rerun during knowledge maintenance.

At completion, licenses/bans/sources passed while the advisory gate remained blocked
by existing Wasmtime 46.0.3 RUSTSEC-2026-0316/0327. Auditing baseline and corrected
lockfiles also found existing Hickory-proto 0.24.4 RUSTSEC-2026-0119, already triaged
in deny.toml. No new package/version or policy ignore was introduced. This evidence
does not establish a full CI/supply-chain PASS or resolve those advisories.

Re-evaluate when adding body consumers, changing replacement dispatch, introducing
WKT/container dependencies, changing memory caps, or expanding template syntax.
