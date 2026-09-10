# B0298 — HTTP OIDC Unit Tests

- Pipeline state: terminal; deferred to D0154 after independent review and adjudication rejected the no-change classification.
- Active anchor: `edgion-gateway`; baseline `84af258d`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/tests.rs`, reopened after external `bea47373` session-refresh JWKS Dye coverage and subsequently used as authorized support by B0293/B0297; earlier B0090/B0091/B0093/B0094 history remains evidence to revalidate.
- Analyst must read the whole test file and trace each material fixture/helper/behavior to production ownership, distinguishing valuable regression coverage from redundant or network/global-state tests. This batch must not alter existing `edgion-tests/**`; any future integration need becomes a standalone issue only.
- User-owned protected paths, campaign records and all technical files are read-only during analysis. No `fix-issue-cn`, commit or push.

## Analysis result

- Fresh child analyst `b0298_oidc_tests_analysis` read all 2,806 lines/61 tests, their production owners, external and B0293/B0297 diffs, network/global-state boundaries, frozen integrations and existing tasks at `84af258d`.
- Decision: examined-no-change. The new paused-time session-refresh JWKS Dye test is high-value and network-free because link-local egress is denied before dial; two Wiremock tests uniquely retain real provider/status/stale adapter coverage. Replacing them needs an unjustified transport seam.
- The sole 100ms real sleep follows `ShardedLruCache`'s standard-Instant behavior and does not justify broader clock abstraction. Splitting/merging shallow or repetitive cases would be churn; focused, ignored and frozen integration coverage supplies the complementary layers.
- D0057/D0058 own existing gaps. No new test or integration issue; keep `tests.rs` unchanged.

## Independent review and adjudication

- Fresh child reviewer `b0298_nochange_review` rejected no-change: two default-run Dye tests in `tests.rs` call `MockServer::start()`, violating the red unit-test rule against network or real servers. Their provider/status/stale/Dye coverage remains load-bearing, so deletion or ignoring is not acceptable.
- Fresh child adjudicator `b0298_adjudication` confirmed defer. The same debt includes 22 default-run network tests in `jwks_provider.rs` (TcpListener fixture, Wiremock and a deliberate loopback refusal), for 24 tests across two files. Link-local requests rejected before dial are not violations.
- D0154 and its standalone task own a plan-first network-isolation refactor that preserves cache, validation, error, singleflight, rotation and Dye contracts without adding request-hot-path cost. D0057 is completed and D0058 is unrelated.
- No technical or integration-test file changed. Root format, locked workspace check, locked Clippy (existing warnings, exit 0), agent-doc, SSA, metrics, Pingora, Gateway tracing, English, frozen-integration and diff gates passed. Commit `f3d7bc5d`; no push.
