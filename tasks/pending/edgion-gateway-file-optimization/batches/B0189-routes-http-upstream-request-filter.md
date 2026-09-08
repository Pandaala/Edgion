# B0189 — HTTP Upstream Request Filter

## Scope

- Analysis baseline: `848e96e9621ab2765ded74b24135d6ccd943a51d`; revalidated after record-only HEAD `22d0e0ff`
- Target blob: `66ba047b962fdda1829557baacc5b667ec7c3c0d`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. Pingora removes `Connection` and nominated fields before this hook, so validating deferred credential targets only against the sanitized attempt header misses a late request-plugin nomination and can reinsert a Secret after sanitization. Local hook preparation failures also use `InternalError`, allowing attempt-owned outlier state to blame the backend for local policy failures.
- Frozen technical scope: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`, `edgion-resources/src/diagnostic.rs`, `skills/04-review/security/regression-guards/deferred-credential-connection-nomination.md`, and `skills/04-review/security/SKILL.md`.
- Required behavior: validate against both the canonical post-plugin session header and sanitized attempt header before secret materialization; use a static nonretryable `HTTPStatus(500)` control-flow error for local preparation failures; retain redacted canonical diagnostics, authority/framing/forwarded/protocol/AI order, and move credential-only clock work under the existing optional branch.
- Deferred candidate: add a later real H1 pipeline integration for CredentialInjector followed by a late `Connection` nomination; existing integration files remain frozen.
- This is a queued candidate and cannot enter the serial mutation/review lane before earlier batches commit.
