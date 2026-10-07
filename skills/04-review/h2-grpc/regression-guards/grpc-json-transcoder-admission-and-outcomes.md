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

## Rationale and evidence

The [approved correction plan](../../../../tasks/pending/grpc-json-transcoding/07-astra-correction-plan.md)
addresses the second Astra review at 4dbcc5495. Canonical context registration and the
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

Resources normalizes bounded JSON before reflection: validate names and aliases first;
remove ordinary null fields, including whole repeated/map fields; preserve singular
and element Value nulls. Count the protobuf/WKT shape, including Struct entries,
ListValue/Value expansion and FieldMask paths, before typed/reflected allocation.
Only the nine exact canonical scalar-wrapper names take the wrapper shortcut;
similarly named ordinary messages in google.protobuf follow full field traversal.
The [memory ledger](../../../../tasks/pending/grpc-json-transcoding/03-implementation-plan.md#memory-and-time-ledger)
includes temporary typed trees, capacities and transcode buffers under existing limits.

Integers use exact bounded decimal/exponent scanning for strings and exclusive upper
bounds for floating values. Finite numeric float strings cannot become infinity.
A borrowed recursive serde adapter renders nonfinite wrappers as canonical strings,
forwards collect_str to the capped writer, and preserves Value's nonfinite rejection.
Binding insertion accepts either parent alias but rejects alias duplication and leaf
collisions. Publication proves same-verb path languages disjoint with one bounded
All-match automaton per verb, including EOI transitions and one aggregate work budget.
Exhaustion fails validation; runtime ambiguity defense remains.

## Verification and re-evaluation

Five new resource regression groups failed on the original code. The corrected
Resources grpc_transcoding tests and Algo regex_disjoint tests passed during development.
Gateway owner/outcome tests, shared-topology negative cases with upstream-count
assertions, and terminal access-log cases accompany the correction. Fresh independent runtime, AI/integration and corrected codec review scopes passed.
Fresh locked workspace check/clippy, 1,058 focused regression tests, eleven guards,
40 live transcoder cases, Responses/native protocol regressions, both hot-reload
scenarios and residue validation passed. The request/response-header-only ExtProc
companion sends request headers because the existing FakeProcessor requires that
phase before responding; both body modes stay NONE and fail-close remains enabled.
Exact fresh commands, first-run failures/corrections and the unchanged advisory
blocker are in the [final verification record](../../../../tasks/pending/grpc-json-transcoding/09-astra-final-verification.md).

Re-evaluate when adding body consumers, changing replacement dispatch, introducing
WKT/container dependencies, changing memory caps, or expanding template syntax.
