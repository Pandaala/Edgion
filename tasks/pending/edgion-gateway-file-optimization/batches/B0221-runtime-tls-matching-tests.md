# B0221 — Runtime TLS Matching Tests

## Scope

- Pipeline state: analysis-ready; queued, not active mutation.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/matching/tests/tls_test.rs`.
- Baseline: `a8f62d29`.
- Analyst: `b0221_tls_test_analysis`.
- Investigate full test coverage, actual matcher/security contracts and relevant production consumers. Preserve existing tests; no integration writes or scope expansion authorized.

## Analysis Outcome

- Analyst recommends examined-no-change at `93a07192`; target blob `215e7c1bd29320dfa68617b9ae10b07ee0eb570f`, matcher blob `692d7b6bd0c1af824c3b8cf938c2fe95f82c9e12`. Fresh TLS matcher tests 26/26 passed in 0.23 seconds; source allow-list empty.
- Full matcher, TLS callback and shared hostname/CA helpers establish strict source identity, aggregate limits, rejection precedence and generation replacement coverage. The 1,032 distinct certificates are necessary to test the deduplicated 1,024-certificate bound; fixture compression offers no demonstrated benefit.
- Basic match fixtures may return a StrictReject entry without serving material; `Some(entry)` alone is not handshake-success evidence. Preserve all existing security assertions.
- At promotion, independently review the no-change decision and a standalone frontend-TLS knowledge follow-up: the TLS architecture index, empty-CA regression pages and matcher-separation table retain obsolete fallback/guard claims; `tls_pingora.rs` rustdoc also misstates strict empty refs. Correct documentation separately, never restore obsolete runtime behavior. No integration changes.
