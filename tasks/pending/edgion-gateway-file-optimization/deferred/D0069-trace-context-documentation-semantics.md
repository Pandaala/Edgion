# D0069 — Correct TraceContext Documentation Semantics

## Meta
- Status: superseded (2026-09-11)
- Severity: medium
- Discovered in: B0105, TraceContext analysis
- Category: documentation-correctness
- Canonical knowledge: [RequestTraceId lifecycle](../../../../skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md)

## Historical evidence
TraceContext Rustdoc and bilingual user guides describe `trustInbound: false` as unconditionally generating a root even though runtime generation still depends on `generateWhenMissing`. They also describe `defaultSampled: true` as forcing all downstream services to sample, while the W3C sampled flag is only a recommendation propagated in trace context.

## Original deferral rationale
The wording must be audited across schema Rustdoc, English/Chinese guides, examples, and any generated reference text. Mixing that documentation sweep into B0105 would expand a two-file runtime correctness batch without affecting its implementation.

## Original integration-test assessment
None. This is documentation parity with existing runtime behavior; no integration issue is needed.

## Resolution

The original todo had already been removed by user-synchronized commit `78a25c3a`; this change repairs its stale campaign pointer and records the replacement contract, rather than claiming that earlier task deletion.

The user-approved replacement removes TraceContext and its trust/generation/sampling options. Core W3C context has one request owner; RequestId rules compose incrementally. Three independent reviews, focused unit/CRD checks, static RequestId integration and RequestIdDynamic passed. The linked knowledge page records the exact contract, verification limits and provenance.
