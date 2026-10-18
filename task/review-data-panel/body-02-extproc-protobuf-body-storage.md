# ExtProc protobuf body representation

Status: deferred for user decision. Source baseline: `2b291895c`.

## Evidence

Three Gateway producers materialize `HttpBody.body` as a Vec:

- `edgion-gateway/src/plugins/http/ext_proc/streamed.rs::on_chunk` copies the
  current request chunk before the awaited body exchange.
- `edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs::on_event`
  checks the response-window input cap, then copies the current window.
- `edgion-gateway/src/plugins/http/ext_proc/plugin.rs` materializes the captured
  buffered request body only after that body phase is admitted. Earlier header
  verdicts can suppress it; this ordering is intentional.

`edgion-common/src/ext_proc/proto/ext_proc.proto` declares
`envoy.service.ext_proc.v3.HttpBody.body` as bytes field 1. Common's build script
uses default Vec generation for ExtProc; its separate conf_sync generation
already selects Bytes for two exact fields. This establishes an available
code-generation mechanism, not approval to change ExtProc ownership.

The client sends owned ProcessingRequest values through a bounded mpsc channel.
The exchange awaits the matching verdict and, on the last phase, clean EOF with
its own bounded drain. Encoding and cancellation therefore outlive the immediate
message constructor. Existing request and response limits are distinct; the
response-window cap is not evidence of an identical request cap.

## Decision and tradeoffs

Investigate an exact-field Bytes mapping for HttpBody.body, or retain the current
Vec representation. Do not use wildcard bytes mapping or include header raw_value,
BodyMutation or ImmediateResponse fields without separate evidence and review.

Cloning an existing Bytes handle could avoid the source-to-Vec copy. It may also
keep a larger sliced backing allocation alive through queued encoding; some Bytes
backing forms allocate on first clone. Codec serialization may still copy bytes.
The current Vec copy has independent storage and different lifetime costs.
No measured workload or codec/backing-lifetime analysis establishes a net gain.
This is not a claim of zero-copy transport or a correctness defect.

A future implementation crosses Common code generation, Gateway producers and
all generated-client/server consumers and tests. Freeze the exact allow-list in
a new IMPL plan after enumerating them; this task authorizes no such edits.

## Required evidence before implementation acceptance

- Compare encoded messages and real processor interoperability for empty, binary,
  fragmented and terminal bodies. Keep field numbers and wire payload unchanged.
- Preserve original chunks on fail-open and keep valid mutation, immediate reply,
  wrong-phase verdict, abandoned request and trailer behavior unchanged.
- Preserve admission-before-buffered-body construction, interactive phase order,
  final half-close, verdict/EOF deadlines, response input/output caps and request
  body policy. No response BodyStorage, full-response aggregation or second cache.
- Measure source-copy allocation savings and retained backing memory separately
  for static, owned and small slices of large allocations, including delayed
  processor reads, timeout and cancellation. Account for queued and encoding
  lifetimes; do not infer end-to-end throughput from constructor allocation alone.
- Run focused producer/owner/protocol tests and applicable shared-topology ExtProc
  integration scenarios, plus normal project checks. Retain opaque-header fidelity.

Original constraints: `skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md`,
`skills/04-review/ext-proc/regression-guards/ext-proc-phase-verdict-matching.md`,
`skills/04-review/ext-proc/regression-guards/opaque-header-fidelity.md`, and root
AGENTS request/response body ownership rules. This task records source inspection;
no representation change, benchmark or fresh interoperability test was performed.
