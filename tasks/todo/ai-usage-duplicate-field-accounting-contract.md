# Establish duplicate-field accounting coverage against the production parser

## Meta

- Created: 2026-10-28
- Status: pending
- Type: investigation

## Evidence

At baseline `75c4d51ecd195193ba6a91d210736c0be4e4676f`, Gateway usage tests compare their legacy visitor with an independent serde_json::Value oracle. Production uses observe_canonical_block. Legacy duplicate choices/delta/content inputs use last-wins semantics; canonical field lookup treats duplicates as ambiguous. For example, duplicate content values null then x contribute one output unit in the legacy parser, while canonical ambiguity prevents that contribution. Canonical tests protect unavailable duplicate-field semantics, but do not demonstrate acceptance of every resulting accounting difference.

## Investigation boundary

Read original repository rules and relevant AI usage/quota/canonical SSE architecture and regression guards. Establish the intended duplicate-field accounting contract independently of current implementation. Map existing numeric, duplicate, escaped-key and cross-product cases to production parsing with independent expected values and protected test granularity. Preserve the legacy parser and six helper macros until equivalence or an explicitly authorized behavioral change is established. Do not rewrite expectations simply to match current code.

## Acceptance

Document intended numeric/output-accounting outcomes, demonstrate production-facing coverage for every retained regression, and identify exact obsolete tests/helpers only after their exclusive ownership and equivalent coverage are proven. No parser or production behavior change is authorized by this investigation.
