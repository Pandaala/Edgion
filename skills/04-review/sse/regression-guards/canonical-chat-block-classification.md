---
name: canonical-chat-block-classification
description: Use when reviewing canonical Chat SSE source eligibility, controls, poisoned semantics, and accounting separation.
knowledge_type: regression-guards
---

# Canonical Chat sources preserve typed block classification

## Conclusion and scope

SemanticSseSource and the Chat UsageExtractionProcessor branch both use
`openai::canonical_sse::observe_canonical_block` with the decoder's status.
Data retains payload validation, including unavailable semantics for explicit
empty data. Dataless comment, ID-only and event-only controls remain available,
contribute no semantic fragments, usage or output units, and preserve raw bytes.
Poisoned blocks retain the existing payload-derived accounting but force
unavailable semantics and clear fragments. Guardrail follows its configured
failure policy for unavailable semantics; passive accounting is independent.

## Rationale and evidence

ARCH-013 identified both sources discarding typed classification and parsing
Dataless empty payloads as JSON. That falsely turned legal provider controls
into Guardrail dependency failure, terminating or disabling protection.
Classification now belongs to one existing synchronous observer seam, before
owned CanonicalRecordBatch handoff to Guardrail and RawWindow. The canonical
[substrate contract](../../../01-architecture/02-gateway/00-overview.md#canonical-sse-substrate)
continues to own raw order, bounded admission and generation resolution.

Poisoned non-data lines with parseable JSON previously could remain semantically
available. Rejecting those fragments is the approved trust change; preserving
usage and output units avoids changing independent accounting. The
[malformed quota contract](../../ai-quota/regression-guards/sse-malformed-events-must-not-disable-enforcement.md)
retains cumulative counting, one-event tolerance, enforcement gates and precedence.
Responses and Anthropic retain their own protocol policies.

## Verification and re-evaluation

Regressions in canonical_sse_test.rs cover typed controls, empty data, poisoned
non-data with valid usage/content, both accounting modes and raw preservation.
usage_test.rs drives the actual usage source with guarding enabled/disabled and
checks reported tokens, output units, one completed poisoned event and raw bytes.
response_test.rs drives both actual sources through mock capability handoff into
Guardrail on_chunk, with split controls, interleaving and EOS. Closed dependency
admission proves controls do not cause failure and later text still reaches
policy admission without network calls. Both fail-open and fail-close are covered.

Fresh execution results belong to the task verification record; test descriptions
alone do not establish passing results or live provider/transport acceptance.
Re-evaluate if decoder status semantics, source modes, quota accounting, or
Guardrail handoff ownership changes. No new parser, cache, response aggregation,
content copy or per-block observability is introduced by this correction.

## Resolution references

- ARCH-013 — canonical Chat block classification correction, 2026-10-12.
