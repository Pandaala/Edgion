# B0166 — gRPC Route Match Engine Precedence

## Scope

- Candidate analysis HEAD: `d99393754bce4cfd761e99ddd3f5e6abb1e5c947`; freshness-revalidated at `55772d157d7a383997dc6042779ecc3f72687ec9` and promoted unchanged after external ACME-only drift at `ff86be8ff1d8615775a44b33be7071dcb8945914`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/match_engine.rs`
- Baseline blob: `9e0997b8172c27170a89f0094c69c0317b7c88f4`
- Exact implementation allow-list:
  - `edgion-gateway/src/routes/grpc/match_engine.rs`
  - `tasks/todo/grpc-route-complete-precedence-and-hostname-ordering.md`
  - `tasks/todo/grpc-route-method-only-match-policy.md`

## Handoff

- Analysis: `b0163_final_review` read the full matcher, match-unit/manager/conf-handler/integration/request call chain, Controller validation, committed Gateway API v1.5 CRD and existing tests. It revalidated the unchanged target after B0164/B0165; the implementer separately confirmed the later external commits touched only ACME/Controller/docs.
- Defect: regex candidates currently compare combined UTF-8 byte length of service and method. Gateway API requires service character count first, then method character count, then headers. A shorter service with a longer method can therefore win incorrectly.
- Selected fix: within existing regex buckets compare service Unicode scalar count descending, then method scalar count descending, then existing specificity. Preserve classification, fixed bucket traversal, deep matching, generation ownership, publication and request-path shape.
- Plan: `b0131_final_review` proposed the build-time lexicographic comparator, direct bucket-order regressions and two atomic deferred tasks. Independent plan reviewer `b0164_final_review` approved it without Must Fix or Should Fix after requiring actual vector-order assertions and a Unicode byte-versus-character reversal case.
- Implementation: regex candidates now compare service Unicode scalar count descending, then method scalar count descending, then the existing specificity order. All exact/regex pattern accessors use character counts; regex-service-only retains service count plus specificity. Classification, seven-tier traversal, deep matching, publication and request execution remain unchanged.
- Tests: 12 focused matcher tests passed. New direct bucket-order coverage proves service priority over a much longer method, method tie-breaking, Unicode character rather than UTF-8 byte count, and existing header/route/rule/match deterministic fallbacks.
- Deferred: `tasks/todo/grpc-route-complete-precedence-and-hostname-ordering.md` records the full hostname and cross-bucket precedence design; `tasks/todo/grpc-route-method-only-match-policy.md` records the required atomic support/rejection decision. Neither is partially implemented here.
- Integration coverage: selected comparator is unit-testable; existing integration files remain frozen.
- Review: fresh independent final reviewer `b0166_final_review` re-read the three-file diff and matcher rules, verified every comparator/bucket/test boundary and returned LGTM with no Must Fix or Should Fix. External HEAD drift to `b179660c` did not touch the target or its call chain.
- Verification: `cargo fmt --all`; focused tests 12/12; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,113-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `bc47034c`; never pushed by the campaign.
