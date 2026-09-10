# B0295 — HTTP OIDC JWKS Provider

- Pipeline state: terminal; changed after approved plan, independent final LGTM and full verification.
- Active anchor: `edgion-gateway`; baseline `309a1c94`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`, reopened after external stage-aware OIDC dye-test support; B0092 history remains evidence to revalidate.
- Analyst must audit the full provider lifecycle, discovery/JWKS consumers, refresh/cache/concurrency behavior, error/redaction paths, tests and existing review/tasks. D0058 owns the broader dependency-error redaction repair and must not be duplicated or implemented opportunistically.
- Existing `edgion-tests/**`, user-owned protected paths, campaign records and all technical files are read-only during analysis. No `fix-issue-cn`, commit or push.

## Analysis result

- Analyst `b0295_jwks_analysis` audited the full provider and OIDC module, cache/singleflight lifecycle, outbound client/TLS/egress/caps, diagnostics, tests, B0092 history and existing tasks at `309a1c94`.
- Concrete defect: JWKS fetch stores `last_refresh_at`, success expiry and stale retry expiry from the pre-network attempt time. A slow fetch consumes the intended post-completion TTL/backoff and can cause immediate serialized retries during an outage.
- Decision: change, moderate but plan-first because it touches authentication cache/concurrency. Exact technical allow-list is `jwks_provider.rs`; use a fresh completion/publication `Instant` for state deadlines and add a deterministic network-free state-transition regression in the same file.
- Preserve ordinary TTL eligibility, force-refresh/double-check behavior, immutable Arc publication, stale/no-cache outcomes, Dye, clients/TLS/egress/caps and failure policy. D0058 remains separate; no integration edit or new issue.

## Plan and implementation

- Implementer `b0295_implementation` returned a no-edit plan: extract the synchronous post-fetch state transition, sample completion after acquiring the state write lock, preserve both eligibility checks and singleflight, and cover success/stale publication deterministically.
- Fresh child reviewer `b0295_plan_review`: approved with no blocking issue. It pinned all-result `last_refresh_at`, exact success/stale deadlines, cached Arc identity, no-cache error behavior, write-lock sampling and unchanged lock ordering.
- The same implementer changed only `jwks_provider.rs`: added private `JwksState::publish_fetch_result`, derived published deadlines from `completed_at`, and added `jwks_publication_deadlines_start_at_completion`.
- Exact network-free test passed 1/1 (4,386 filtered out); scoped rustfmt and diff checks passed. No integration, outbound, error/Dye, singleflight or cancellation behavior changed.
- Fresh child final reviewer `b0295_final_review`: LGTM, no MustFix/ShouldFix/deferred finding. It independently verified all three result branches, completion-time sampling, deadlines/Arc/outcomes, unchanged eligibility and lock/singleflight/cancellation behavior, deterministic test coverage, one-file containment and D0058 separation.
- Root format, locked workspace check (23.40s), locked Clippy (10.06s rerun; existing warnings, exit 0), agent-doc, SSA, metrics, Pingora, Gateway tracing, English, frozen-integration and diff gates passed.
- Ledger/changelog updated. Commit pending automatic local commit; no integration edit or push.
