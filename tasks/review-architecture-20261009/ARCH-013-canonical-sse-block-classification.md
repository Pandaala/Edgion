---
id: ARCH-013
status: candidate
kind: behavior-defect
priority: P2
primary_partition: PART-245
independent_verification: confirmed
---

# Preserve typed SSE classification at canonical semantic handoff

## Evidence and ownership gap

The canonical SSE decoder distinguishes Data, valid Dataless control blocks,
and Poisoned framing/text. A comment-only `b": ping\n\n"` block is Dataless;
its semantic_data payload is empty. Two Chat producers discard that status:
SemanticSseSource observes every block, and the Chat UsageExtractionProcessor
branch likewise observes the payload regardless of block status. The separate
Responses branch explicitly handles Dataless.

Empty JSON parsing yields semantic_available=false. CanonicalRecordBatch's
push_observed marks the batch unavailable; response Guardrail treats that as
a dependency failure. Default fail-close terminates the stream; fail-open releases
bytes and disables guarding. The existing comment/EOS Guardrail test constructs
raw records directly and bypasses this source handoff. These are static source
conclusions; no stream or callout was executed.

The decoder owns classification, but the semantic observer accepts only the
payload. Producers must reconstruct which records need JSON observation; two
paths lose the same fact while another preserves it. New sources would repeat
the same distributed policy. This is a typed-to-untyped ownership gap, not a
request to merge protocol implementations.

## Bounded recommendation and alternatives

Make the canonical Chat record-observation seam status-aware and use it from
both producers. Data receives JSON observation; Dataless retains raw slices
without semantic fragments or usage; Poisoned explicitly carries unavailable
semantic status. Poisoned handling is a proposed typed contract, not a universal
description of current behavior: a poisoned non-data line alongside valid JSON
data can currently produce an available payload observation.

A caller-local match fixes each occurrence but leaves classification policy
distributed. A small canonical helper preserves the decoder's existing fact
without another decoder, cache, window or content allocation. Keep semantic
source availability separate from the existing cumulative malformed-SSE quota
threshold and its termination gates.

An actual empty data field is still Data and retains its current JSON/protocol
validation behavior. Do not drop raw control records: comments, IDs and framing
must remain byte-identical, ordered and subject to existing capacity limits.

## Invariants and future validation

Dataless controls contribute no usage/text and do not disable Guardrail. Preserve
empty-data failures, degraded/opaque behavior, independent batch/window bounds,
EOS resolution, cancellation and terminal ownership. Explicit poisoned source
handling must not change the separate quota poison tolerance, precedence or
termination gates. No per-block log or metric is needed.

Future regressions should drive comment-only, ID-only and event-only blocks
through actual semantic-only and usage-enabled Chat sources, including splits
and interleaved content. Assert available batches, exact raw output, continued
guarding and no callout solely for controls. Pair empty data and poisoned cases,
including poisoned non-data lines with otherwise valid data, to protect typed
classification. Exercise source-to-window wiring rather than manually
constructing raw records.

## Original context and limits

All seven OpenAI and five Anthropic protocol primaries and ten protocol test
files were fully read by the analyst. Original empty-data, canonical SSE/window,
malformed-SSE quota and Guardrail contracts distinguish absent data from
dispatched empty data and preserve SSE control bytes. Supporting seams and hashes
are in the partition report and STATE.json. No implementation, tests or fresh
wire validation were performed; independent verification is pending.
ARCH-010 concerns JSON grammar after classification and has a separate root.
