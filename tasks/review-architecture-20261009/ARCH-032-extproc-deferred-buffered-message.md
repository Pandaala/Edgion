---
id: ARCH-032
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-309
analyst: /root/analyze_conversion
independent_verification: VERIFICATION-ARCH-032.json
---

# Prepare the buffered ExtProc body message after its headers verdict

## Current ownership and concrete cost

`ExtProc::run_request` reads the canonical request body before sending any
phase (`plugin.rs:719`), constructs a two-element Vec<ProcessingRequest>
(`:727`) and copies captured Bytes into HttpBody.body with body.to_vec()
(`:739`). The loop then exchanges and validates request headers (`:757`),
applies the mode override/verdict, and decides whether the body is needed.
BUFFERED reduction to NONE skips the prebuilt body (`:752`);
CONTINUE_AND_REPLACE stops the request-body direction (`:809`). ImmediateResponse,
unusable verdict or dependency failure also returns before that body is sent.

For SEND headers plus a nonempty BUFFERED body, a valid reduction to NONE,
replacement or immediate response therefore discards an O(body length) owned
protobuf copy created before the headers await. This is a concrete unnecessary
copy and retention interval, not measured allocator/RSS/throughput cost.
Successful body exchanges still need the Vec under the generated wire type.

The actual production wrapper is ConditionalRequestFilter around
BodyGatedRequestFilter around ExtProc when a resolved body requirement exists
(`plugin_runtime.rs:1399`). Materialization and early storage-read failure
semantics are canonical. This finding concerns the additional protobuf copy.
Existing dynamic_request_header_reductions_cleanly_half_close_live_streams
test source covers a buffered request with a prebuilt body and asserts only
one phase is sent with clean half-close; it does not measure allocations.

## Bounded improvement and alternative

Retain the already-read immutable Bytes locally. Exchange headers first when
configured, then construct RequestBody only if its original direction continues
and the validated effective mode remains BUFFERED. Copy those retained bytes
immediately before body exchange. This preserves the captured generation and
early failure path. Moving the entire body read after headers would change
storage/body-gate policy and is not this recommendation.

A private lazy phase descriptor is another option, but explicit header/body
orchestration is small enough here. Add no generic lazy-message framework,
second cache, resource schema change or shared-body implementation.

## Required invariants and future verification

Preserve one AttachedExtProc owner and Process stream, early body read and
failure policy, per-phase matching and reduction-only overrides. Validate the
headers verdict before disclosing body bytes. Preserve original header EOS
without predicting a future reduction. Preserve outbound half-close and
separately bounded inbound final EOF drain, future response phases, attachment
identity, conditional/dye behavior and canonical aggregate access logs.

CONTINUE_AND_REPLACE must publish through canonical BodyStorage before later
header mutation; publication failure terminalizes the owner under both failure
policies. Retain byte-exact ImmediateResponse/local replies. Do not alter
STREAMED request or response handling, TLS/egress, PluginSession or transport facts.

Future verification should cover buffered continuation, headers skipped, NONE
reduction with/without future response phases, replacement, immediate/malformed
verdicts, publication failure, cancellation and clean half-close. Any allocation
claim needs a distinguishing check that the unsent body message is never built.

## Original decisions, deduplication and limits

Personally read and revalidated interactive Continue-and-Replace, phase verdict
matching, request-body gate lazy materialization, atomic mutation physical caps,
clone-cheap local response Bytes and transcoder admission records. Ordered
interactive sending does not require prebuilding unsent payloads; clone-cheap
processor-to-client replies are different from this outbound request Vec.
No task/decision requiring eager protobuf construction was found. This is
separate from ARCH-001 session vocabulary, ARCH-003 terminal preparation and
ARCH-005 transport metadata. Static source/test analysis only: no code change,
tests, builds, benchmark, incident or live interoperability proof. Independent
verification is pending.
