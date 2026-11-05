---
name: access-log-request-uri-include-raw-uri-alloc
description: Preserve gated frozen URI projection and avoid an unconditional request cache; evaluate serialization alternatives using measured cost and canonical masking.
knowledge_type: intentional-design
---

# Access-log raw URI ownership and optimization boundary

## Current evidence

AccessLogEntry::from_context in edgion-gateway/src/observe/access_log/entry.rs
populates request_uri only when includeRawUri is enabled and entry_raw_path is
nonempty. EdgionHttpContext::request_uri delegates to compose_uri in ctx.rs,
joining the frozen entry path and optional entry query into an owned String.
It does not include scheme/authority or use the current rewritten URI. Only an
absent query omits the question mark; a present-empty query retains it.

The field currently uses Option<String> and the canonical masked_opt serializer.
Sensitive mode is applied when serializing, not when constructing the entry.
AccessLogEntry::to_json produces a separate owned JSON string while the context
is alive. Terminal logging sends that owned output to its sink; the debug-header
filter also serializes before mutating the response. Debug Wrap mode suppresses
the header before building a snapshot.

Most context projections borrow. They are not all borrowed: GeoSlotLog can own
a formatted coordinate String, and terminal enrichment has its own ownership.
The request_uri representation is current implementation, not a requirement
that the JSON string field must be backed by its own intermediate allocation.

## Retained design boundary

The historical review rejected adding RequestInfo::entry_raw_uri merely to move
the same composition into request entry. That remains a sound reason against
this cache proposal: it need not reduce allocations, extends retained state,
and an unconditional cache penalizes requests with includeRawUri disabled.
The plugin get_request_uri API separately returns an owned String; changing a
logging projection does not require changing that API.

Keep serialization within the context lifetime and keep the final owned JSON
output. Do not introduce a broad context cache, deferred context serialization,
buffer pool or per-request instrumentation under this finding.

## Corrected interpretation of the historical review

The former page asserted typical JSON/URI sizes, dominant serialization cost and
that the URI allocation was invisible in any profile, without measurement
supporting those claims. Those statements do not establish current performance.
The sibling access-log-json-serialization-per-request evidence page describes
the final-output ownership rationale, but its empirical cost claims are not
evidence that this intermediate URI copy is negligible.

No fresh workload profile or throughput measurement was collected for this
source audit. A narrower fragment projection could avoid intermediate assembly,
but the current canonical apply_sensitive API requires contiguous text.
Whether and how to extend that mechanism needs design and verification; this
page does not authorize independent masking logic or promise zero allocations.

Re-evaluate proposals against frozen URI bytes, JSON escaping, omission rules,
serialization-time Off/Replace/Wrap behavior, failure fallback and final-output
ownership. A measured, bounded serialization proposal must not be dismissed
solely because the final JSON also allocates. The proposal was
the historical task `log-01-raw-uri-fragment-serialization` (declined in the 2026-11-05 data-plane triage; the record survives in git history at `a59c3da81`).
