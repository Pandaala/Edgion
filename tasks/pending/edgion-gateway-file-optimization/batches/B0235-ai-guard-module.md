# B0235 — AI Guard Module

## Scope

- Pipeline state: analysis-ready; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `e42437af`.
- Target: `edgion-gateway/src/plugins/http/ai_guard/mod.rs`.
- Inspect the complete module facade, exports, compilation/runtime ownership and actual consumers. B0234 separately owns evaluator request serialization.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after B0234 before promotion.

## Analysis Outcome

- Analyst `b0235_ai_guard_module_analysis` recommends examined-no-change at `e42437af`; target blob `584fcb2b49d6bb818d4d0849af4b8efb6c86d12c`. The eight-line facade declares four private implementation modules and exposes only `AiGuard`; actual compilation, body handling, evaluator I/O, failure policy and access-log summaries live in siblings.
- Both module-scoped and flat `AiGuard` paths follow the existing HTTP plugin pattern and have live runtime consumers. Removing exports creates import churn; exposing internals widens API; merging files changes no cost or coupling; direct facade tests would exercise no behavior.
- Existing D0082 owns cross-entry AiGuard/AiProxy publication ordering; do not duplicate it. No new issue, documentation or integration change.
- Suggested fresh filter: full `plugins::http::ai_guard` tests, then standard gates. Revalidate target blob/export chain after B0234's private evaluator change before independent final review.
- Original plugin architecture/development, BodyStorage, outbound cap/client/egress/TLS, failure/redaction/access-log/observability, Rust/testing knowledge and D0082 were independently applied. All target-local hard gates are vacuous or preserved.
