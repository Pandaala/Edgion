# B0189 — HTTP Upstream Request Filter

## Scope

- Analysis baseline: `848e96e9621ab2765ded74b24135d6ccd943a51d`; revalidated after record-only HEAD `22d0e0ff`
- Target blob: `66ba047b962fdda1829557baacc5b667ec7c3c0d`
- Pipeline state: ready to commit
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. Pingora removes `Connection` and nominated fields before this hook, so validating deferred credential targets only against the sanitized attempt header misses a late request-plugin nomination and can reinsert a Secret after sanitization. Local hook preparation failures also use `InternalError`, allowing attempt-owned outlier state to blame the backend for local policy failures.
- Frozen technical scope: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`, `edgion-resources/src/diagnostic.rs`, `skills/04-review/security/regression-guards/deferred-credential-connection-nomination.md`, and `skills/04-review/security/SKILL.md`.
- Required behavior: validate against both the canonical post-plugin session header and sanitized attempt header before secret materialization; use a static nonretryable `HTTPStatus(500)` control-flow error for local preparation failures; retain redacted canonical diagnostics, authority/framing/forwarded/protocol/AI order, and move credential-only clock work under the existing optional branch.
- Deferred candidate: add a later real H1 pipeline integration for CredentialInjector followed by a late `Connection` nomination; existing integration files remain frozen.
- This is a queued candidate and cannot enter the serial mutation/review lane before earlier batches commit.
- Previous batch: `1aad294d`; never push.
- Freshness: revalidated at `1aad294d6933384413c943a51babb035dba679e2`; target blob remains `66ba047b962fdda1829557baacc5b667ec7c3c0d`. B0188 changed URI write/latch order before connection and did not affect header sanitization, credential state, retry ownership, or this implementation brief.
- Deferred record: `D0107` owns later real H1 sanitizer-pipeline integration coverage; existing integration files remain frozen.

## Approved Implementation Plan

1. Introduce one static, explicitly non-retryable local-preparation error shape.
2. Validate every `Connection` value in both the canonical session request and the sanitized attempt request before credential materialization.
3. Keep expiry and credential application inside the optional deferred-credential branch, preserving the existing preparation order.
4. Add the closed `CredentialConnectionConflict` diagnostic while retaining `CredentialExpired` for expiry only.
5. Cover the full hook, helper edge cases, and diagnostic wire value without changing frozen integration tests.

Independent plan review approved this plan without a Must Fix. Independent final code review reported LGTM with no Must Fix or Should Fix; it confirmed the four-file technical allow-list, lifecycle ordering, static redacted failure shape, retry/outlier ownership, and frozen integration boundary.

## Outcome

- Implementation: the final hook now checks both canonical and attempt-local `Connection` fields before materializing a deferred credential, rejects malformed or nominated values with `CredentialConnectionConflict`, preserves expiry-only `CredentialExpired`, and maps all local preparation failures to one static explicitly non-retryable HTTP 500. Credential-free requests avoid credential clock and application work.
- Tests: target 13/13, diagnostic 2/2, and full proxy-http 261/261.
- Gates: `cargo fmt --all`, locked workspace check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration-path checks passed.
- Commit: pending; never push.
