---
name: oidc-outbound-san-mismatch-tracing-removed
description: Use when reviewing OIDC outbound TLS SAN-pinning findings that report (or propose re-adding) a `tracing::warn!` in `note_oidc_san_mismatch` / the `handle_callback` discovery arm; covers the Iron-rule-3 removal and why the counter is the only channel on the five PluginLog-less outbound call paths.
status: fixed
finding_id: EDG-GW-001
closed: 2026-07-26
knowledge_type: regression-guards
---

# OIDC outbound SAN-mismatch hot-path `tracing::warn!` removed — FIXED

## Conclusion

`note_oidc_san_mismatch` (`edgion-gateway/src/plugins/http/openid_connect/token_exchange.rs`) and
the `handle_callback` discovery-error arm (`callback.rs`) emitted `tracing::warn!` on the request
hot path. Both were removed on 2026-07-26. The helper now increments
`edgion_gateway_outbound_tls_san_mismatch_total{plugin="openid_connect"}` and nothing else; the
callback arm now uses the same channel as its already-correct sibling in `request_filter_impl.rs` —
`session.record_error(EdgionErrorCode::OutboundTlsSanMismatch)` + `plugin_log.push_err("outbound tls san
mismatch")` + `plugin_log.push_fmt("san {}", …)`. Note it is not a full mirror: the sibling also has
an `else` arm (`push_fmt("discovery fetch fail {}", …)`) for **non**-SAN discovery errors, which the
callback arm still lacks — a pre-existing gap left untouched, not part of this finding.

Fix suggestions of the form "keep the warn behind a `// tracing-allow: rare, config-error-only`
marker", "downgrade it to `tracing::debug!`", or "re-add it because the counter loses the `call` and
`reason` detail" are **not accepted**.

## Core Rationale

**1. All five callers are on the request hot path — no background caller exists.**

`note_oidc_san_mismatch` is reached from `exchange_code_for_token`, `refresh_access_token`,
`revoke_token` (`token_exchange.rs`), `verify_token_via_introspection` (`token_verify.rs`), and
`fetch_userinfo_json` (`userinfo.rs`). Every one of those is driven synchronously by `run_request` /
`handle_callback`. Iron rule 3 (`skills/03-coding/01-log-safety.md`) forbids `tracing::*` at **any**
level there. `warn!` is worse than the usual `debug!` re-report: it is emitted at the default level,
so a single misconfigured `validation.subjectAltNames` floods the gateway log at request rate and
charges the subscriber's formatting/write cost to the request thread.

**2. The counter is a sufficient signal for this failure class, by design.**

A SAN mismatch is a static configuration error that fails every request identically — it is not a
per-request anomaly worth a per-request log line. `docs/en/release-notes/2026-06-30-outbound-tls-san-pinning.md`
already names `edgion_gateway_outbound_tls_san_mismatch_total` as the intended alerting signal on
all paths. Losing the `call` and `reason` fields costs nothing for that use: the operator's next
step is `openssl s_client` against the provider, not log archaeology.

**3. Threading a typed error up to a `PluginLog`-owning frame is a real signature cascade, not a
narrow fix.**

It would need a new `OidcVerifyError` variant plus signature/branch changes across
`exchange_code_for_token`, `refresh_access_token`, `verify_token_via_introspection`,
`revoke_token` (result is discarded by `let _ =` in `maybe_revoke_tokens_on_logout`), and
`fetch_userinfo_json` (returns `Option<String>`, so it carries no error channel at all). That is a
plugin-internal API change, not a review fix. The pre-existing doc comment on the helper had already
recorded this rejection; the comment was rewritten to keep the rationale while dropping the claim
that the log line is an observability channel.

**Precision note:** four of the five paths reach neither the session nor the `PluginLog`.
`verify_token_via_introspection` (`token_verify.rs`) is the exception — it already takes a
deliberately-unused `_session: &mut dyn PluginSession` (present only so its signature lines up with
`verify_jwt_token` for the `verify_token` dispatch), so `_session.record_error(
EdgionErrorCode::OutboundTlsSanMismatch)` there would be a genuine one-liner with no cascade. It was
left out of this finding on purpose: it would stamp a per-request status on exactly one of the five
outbound paths for no operational gain, and asymmetric partial stamping is worse than none. A
proposal to stamp **all** reachable paths uniformly is a separate, legitimate task — not a re-open
of this finding. (The release-note scope note was reworded in the same change to say these paths
"own no plugin log" rather than the previously-inaccurate "reach neither the session nor the plugin
log".)

**4. The callback arm had a session and was simply using the wrong channel.**

`handle_callback` takes `plugin_log: &mut PluginLog` and already called
`session.record_error(EdgionErrorCode::OutboundTlsSanMismatch)` next to the warn — so the correct
data-plane channel was one line away, and the identical `JwksProviderError::TlsSanMismatch` arm in
`request_filter_impl.rs` was already written that way. This was channel drift between two copies of
the same branch, not a missing capability.

## Fix Suggestions Not Accepted

- Re-add any `tracing::*` (including `debug!` or a separate `target:`) in `note_oidc_san_mismatch`
  or the `handle_callback` discovery arm — Iron rule 3 is on the path class, not the level.
- Register the warn as a `// tracing-allow:` exception — the sanctioned exception set is
  construction/once-init/`is_integration_testing_mode()` paths; "rare because it needs a
  misconfiguration" is exactly the framing rejected in
  [tls-route-plugin-access-log.md](../mandatory-design/tls-route-plugin-access-log.md) and
  [real-ip-run-request-tracing-removed.md](real-ip-run-request-tracing-removed.md).
- Add a `call` label (`token_exchange` / `token_refresh` / `revoke_token` / `introspection` /
  `userinfo`) to `record_outbound_tls_san_mismatch` to recover the lost dimension — it would make
  the label set asymmetric with the `ldap_auth` emitter for a dimension no alert or runbook uses.
  If a concrete ops need appears, that is a metrics-inventory change via `tasks/todo/`.
- Thread `EdgionErrorCode::OutboundTlsSanMismatch` out of the outbound call paths — see rationale 3,
  including the `verify_token_via_introspection` precision note.

## Known gate coverage gap

`cicd/checks/check_gateway_no_tracing.py` scans `plugins/http/*/plugin.rs`,
`request_filter_impl.rs`, and `request_stage.rs` — `token_exchange.rs`, `token_verify.rs`,
`userinfo.rs`, and `callback.rs` are **not** in `DP_GLOBS`, so this violation was
invisible to the gate and had to be found by review. Same gap class as the wasm finding
([wasm-request-path-tracing-removed.md](wasm-request-path-tracing-removed.md)). Audit these files by
inspection. (`jwks_provider.rs` joined the gate's scan set on 2026-07-26 — see
[oidc-jwks-provider-tracing-relocated.md](oidc-jwks-provider-tracing-relocated.md). The remaining
four are not added because `token_exchange.rs` names `tracing::*` in a doc comment, which the
line-based scanner cannot distinguish from a call.)

## Deferred at the time, since closed

`jwks_provider.rs` kept five `tracing::*` sites (discovery-fetch / JWKS-fetch / stale-cache `warn!`
plus two `info!`) because `get_or_fetch_discovery` is dual-use — driven both by the background
prewarm task in `plugin.rs` and by a request-path lazy fetch. Closed separately on 2026-07-26: four
sites deleted, `"OIDC discovery document loaded"` relocated to `spawn_discovery_prewarm`, and the
prewarm-exhaustion warn (`plugin.rs`) reworded to stop citing the deleted lines. `get_jwks` turned
out to have **no** cold caller at all. See
[oidc-jwks-provider-tracing-relocated.md](oidc-jwks-provider-tracing-relocated.md); a re-report
scoped to `jwks_provider.rs` is that finding, not a re-open of this one.

## Re-evaluation Triggers

Re-open only if:

- The five outbound call paths gain a `PluginLog` channel (e.g. a plugin-wide request context),
  making a bounded per-request signal expressible off `tracing`.
- `edgion_gateway_outbound_tls_san_mismatch_total` is removed or stops carrying the `plugin` label.

## Reference Cases

- Source issue: `tasks/review-issues/gateway-01-oidc-hotpath-tracing-warn.md` (EDG-GW-001, closed
  2026-07-26).
- Files: `edgion-gateway/src/plugins/http/openid_connect/{token_exchange.rs,token_verify.rs,userinfo.rs,callback.rs}`.
- Correct-channel reference: `openid_connect/request_filter_impl.rs` `UnauthAction::Auth` discovery
  arm; `ldap_auth/plugin.rs` `LdapAuthError::TlsSanMismatch` arm.
- Docs updated with the fix: `docs/{en,zh-CN}/release-notes/2026-06-30-outbound-tls-san-pinning.md`
  scope note + SSL-log section; `edgion-tests/integration/conf/EdgionPlugins/OpenidConnect/SanMismatch/README.md`.
- Sibling removals: [real-ip-run-request-tracing-removed.md](real-ip-run-request-tracing-removed.md),
  [wasm-request-path-tracing-removed.md](wasm-request-path-tracing-removed.md),
  [tls-route-plugin-access-log.md](../mandatory-design/tls-route-plugin-access-log.md),
  [dsl-vm-execute-tracing-declined.md](../mandatory-design/dsl-vm-execute-tracing-declined.md).

## Resolution references

- `EDG-GW-001` — 2026-07-26.
