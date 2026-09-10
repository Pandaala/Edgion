# B0295 — HTTP OIDC JWKS Provider

- Pipeline state: analysis-ready; read-only change candidate, not yet in the serial mutation lane.
- Active anchor: `edgion-gateway`; baseline `309a1c94`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`, reopened after external stage-aware OIDC dye-test support; B0092 history remains evidence to revalidate.
- Analyst must audit the full provider lifecycle, discovery/JWKS consumers, refresh/cache/concurrency behavior, error/redaction paths, tests and existing review/tasks. D0058 owns the broader dependency-error redaction repair and must not be duplicated or implemented opportunistically.
- Existing `edgion-tests/**`, user-owned protected paths, campaign records and all technical files are read-only during analysis. No `fix-issue-cn`, commit or push.

## Analysis result

- Analyst `b0295_jwks_analysis` audited the full provider and OIDC module, cache/singleflight lifecycle, outbound client/TLS/egress/caps, diagnostics, tests, B0092 history and existing tasks at `309a1c94`.
- Concrete defect: JWKS fetch stores `last_refresh_at`, success expiry and stale retry expiry from the pre-network attempt time. A slow fetch consumes the intended post-completion TTL/backoff and can cause immediate serialized retries during an outage.
- Decision: change, moderate but plan-first because it touches authentication cache/concurrency. Exact technical allow-list is `jwks_provider.rs`; use a fresh completion/publication `Instant` for state deadlines and add a deterministic network-free state-transition regression in the same file.
- Preserve ordinary TTL eligibility, force-refresh/double-check behavior, immutable Arc publication, stale/no-cache outcomes, Dye, clients/TLS/egress/caps and failure policy. D0058 remains separate; no integration edit or new issue.
