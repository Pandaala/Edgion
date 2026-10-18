# Raw URI fragment serialization

Status: deferred for user decision. No runtime change.
Source baseline: 061b1375e.

## Evidence and cost boundary

AccessLogEntry::from_context conditionally calls EdgionHttpContext::request_uri
when includeRawUri is enabled and the entry raw path is nonempty. compose_uri
allocates a String for the frozen entry path plus a nonempty entry query.
The entry then serializes this field through masked_opt into an independently
owned JSON output. The debug-header filter uses the same projection outside
Wrap mode; terminal logging is another consumer.

This establishes an intermediate URI assembly, not a measured throughput
bottleneck. The relative cost depends on mode, URI size, enabled projections
and how often snapshots are constructed. Adding an eager request URI cache
would move composition and retain more state, not inherently remove it.
The current canonical masking API takes a contiguous &str, so a fragment-based
serializer cannot simply bypass it and claim equivalent behavior.

## Decision alternatives

- Retain the gated current representation unless bounded offline measurement
  shows material benefit.
- Evaluate a borrowed frozen-path/query projection that emits one JSON string
  without a separately composed URI, with a reviewed canonical masking design.
  Compare construction/serialization CPU, allocations and retained memory for
  absent/empty/long queries and Off/Replace/Wrap modes. Include final JSON
  allocation and debug versus terminal use when interpreting measurements.

This is a choice about value and cross-cutting serializer ownership, not evidence
that the existing result is incorrect. The four-file documentation batch makes
no runtime or schema change. Any implementation requires its own IMPL review
and exact source allow-list, including any necessary resources-layer support.
Do not create a local alternative to canonical sensitive handling.

## Acceptance for any selected implementation

Preserve the includeRawUri and nonempty entry-path gates; absent/empty query
must omit the question mark. Preserve frozen entry bytes independently of live
rewrites and exclude absolute-form scheme/authority. Output must remain one
JSON string with identical escaping for separators, percent-encoded bytes,
quotes, backslashes and permitted multi-byte characters.

Preserve Off, Replace and Wrap for the complete composite value, rather than
masking path and query independently. Preserve mode selection at serialization;
moving it to entry construction changes behavior if the mode changes between
those operations. Preserve canonical wrapper/replacement configuration and
failure behavior, including serializer failures and AccessLogEntry fallback.
Debug Wrap must still suppress before snapshot construction. Do not add a debug
header cap or alter its separately accepted development-surface limitations.

Keep context borrows within synchronous serialization and retain the final
owned JSON required by consumers. Do not change the plugin-owned get_request_uri
API, add an unconditional entry cache, defer context serialization across the
logger boundary, or introduce a buffer pool. Tests must compare actual JSON
bytes and failure/omission behavior, not only semantic JSON equality or counts.
Measure allocations separately from test fixture and expected-string creation.

## Knowledge reconciliation

The owning raw-URI knowledge page now distinguishes the valid cache objection
from unsupported claims of negligible cost. The adjacent
skills/04-review/cpu-memory/review-evidence/access-log-json-serialization-per-request.md
also contains historical size/latency/dominance claims not remeasured here;
they remain a follow-up evidence audit, not proof against this proposal.
No new runtime acceptance is inferred from those statements.

Relevant original rules:
- skills/03-coding/observability/00-access-log.md
- skills/04-review/cpu-memory/intentional-design/access-log-request-uri-include-raw-uri-alloc.md
- skills/04-review/observability/intentional-design/debug-access-log-development-surface.md
- skills/04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md
