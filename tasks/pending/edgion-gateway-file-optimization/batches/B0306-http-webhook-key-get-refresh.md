# B0306 — HTTP WebhookKeyGet Refresh

## Scope

- Baseline: `26e7ec7f`.
- Pipeline state: analysis-ready at `26e7ec7f`; B0305 retains the serial lane.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/plugins/http/webhook_key_get/plugin.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- External `b297af6e` added canonical outbound-request diagnostics emission to the WebhookKeyGet request path. Prior B0117 evidence predates this change.
- The analyst must revalidate webhook LinkSys resolution, prepared templates and overrides, outbound diagnostics ownership/redaction, key extraction, default/failure behavior, request-context mutation, observability restraint and deterministic tests before deciding change, examined-no-change or defer.
- Existing integration tests remain frozen. This candidate may not enter implementation, final review, verification or commit before B0305 completes.

## Analysis result

- Analyst `b0306_webhook_key_get_analysis` revalidated the target and its LinkSys resolver, prepared templates, current-generation lookup, outbound diagnostics, key extraction, runtime publication and tests.
- Decision: examined-no-change. The one-byte diagnostics collector is stack-local per variable, contains no names/values/errors, is emitted once before outcome selection, and preserves serial context/template ordering, fallback/skip behavior and canonical PluginLog ownership.
- Canonical outbound client/TLS/egress/redirect, response caps, immutable publication and bounded observability remain intact. No test or integration execution occurred during analysis.
- A separate already-pending resolver row owns removal of the obsolete compatibility argument and avoidable path-override copy; B0306 does not expand into that work and creates no deferred task.
