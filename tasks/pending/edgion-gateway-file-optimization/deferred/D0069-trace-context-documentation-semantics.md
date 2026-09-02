# D0069 — Correct TraceContext Documentation Semantics

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0105, TraceContext analysis
- Category: documentation-correctness
- Canonical task: [`tasks/todo/correct-trace-context-documentation-semantics.md`](../../../todo/correct-trace-context-documentation-semantics.md)

## Evidence
TraceContext Rustdoc and bilingual user guides describe `trustInbound: false` as unconditionally generating a root even though runtime generation still depends on `generateWhenMissing`. They also describe `defaultSampled: true` as forcing all downstream services to sample, while the W3C sampled flag is only a recommendation propagated in trace context.

## Why deferred
The wording must be audited across schema Rustdoc, English/Chinese guides, examples, and any generated reference text. Mixing that documentation sweep into B0105 would expand a two-file runtime correctness batch without affecting its implementation.

## Integration-test impact
None. This is documentation parity with existing runtime behavior; no integration issue is needed.
