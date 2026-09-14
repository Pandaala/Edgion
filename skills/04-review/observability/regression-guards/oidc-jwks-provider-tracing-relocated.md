---
name: oidc-jwks-provider-tracing-relocated
description: Use when reviewing OIDC `jwks_provider.rs` tracing findings, or proposals to re-add a discovery/JWKS fetch log line, split the provider into cold/hot entry points, or thread a caller-kind flag through `get_or_fetch_discovery` / `get_jwks`; records why four sites were deleted, why the fifth moved to the prewarm task, and that `get_jwks` has no cold caller.
status: fixed
finding_id: oidc-jwks-provider-dual-use-tracing
closed: 2026-07-26
knowledge_type: regression-guards
---

# OIDC `JwksProvider` dual-use tracing — FIXED (4 deleted, 1 relocated to the cold caller)

## Conclusion

The request-reachable provider remains free of tracing. Only the background
prewarm caller logs discovery success, with a static message and component label:
issuer, JWKS URI, discovery URL, and raw dependency errors are not logged.
Discovery failures now have a 5-second backoff and data-free error classifications;
see [dependency error projection](../../oidc/regression-guards/dependency-error-projection.md)
and [discovery failure backoff](../../oidc/regression-guards/discovery-failure-backoff.md).

## Historical relocation evidence (2026-07-26)

The original payloads and scanner behavior below describe the July change, not
current authorization to log endpoint values or a claim that discovery lacks
backoff. The cold/hot ownership rationale remains applicable.

`edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs` held five `tracing::*` sites
inside `get_or_fetch_discovery` / `get_jwks`, both request-path reachable via lazy fetch. All five
are gone from the provider:

- **Deleted** (four): the discovery-fetch SAN `warn!`, the JWKS-fetch SAN `warn!`, the
  `"JWKS refreshed successfully"` `info!`, and the `"JWKS fetch failed, serving stale cache"`
  `warn!`.
- **Relocated** (one): `"OIDC discovery document loaded"` now fires in `spawn_discovery_prewarm`
  (`plugin.rs`), the provably cold caller, which already owns the `Result`.

Both `record_outbound_tls_san_mismatch` call sites also switched from a hardcoded
`"openid_connect"` to `self.config.component_label`, and `jwks_provider.rs` joined
`check_gateway_no_tracing.py`'s scan set.

Fix suggestions of the form "split each function into a cold and a hot entry point", "thread a
caller-kind enum / `is_cold` flag down", "move all lazy fetching off the request path", or "re-add
the stale-cache warn because it is throttled anyway" are **not accepted**.

## Core Rationale

**1. The premise that both functions are dual-use is wrong — only `get_or_fetch_discovery` is.**

The originating todo assumed a "background JWKS refresh" consumer. There is none.
`JwksProvider::get_jwks` has exactly one caller — `discovery_jwks.rs::get_or_fetch_jwks` — which is
called only from `token_verify.rs` (`verify_jwt_token_internal`, initial lookup + `kid`-miss
force-refresh). Both are inside JWT verification on the request path. So the three JWKS-side sites
were **pure hot path**, identical in class to the already-closed
[oidc-outbound-san-mismatch-tracing-removed.md](oidc-outbound-san-mismatch-tracing-removed.md), and
needed no special handling at all. Only `get_or_fetch_discovery` has a genuine cold caller
(`spawn_discovery_prewarm`) alongside six request-path call sites across five files:
`request_filter_impl.rs`, `callback.rs`, `token_verify.rs` (×2), `userinfo.rs`, and
`token_exchange.rs` (`try_refresh_session_token`) — plus one internal call from `get_jwks` itself.

Verify this before re-reporting: `rg -n 'get_or_fetch_jwks|get_jwks|get_or_fetch_discovery'
edgion-gateway/src`.

**2. Every deleted site was already covered by a non-tracing channel.**

| Deleted site | Channel that already carried it |
|---|---|
| discovery SAN `warn!` | `record_outbound_tls_san_mismatch()` fires unconditionally on the adjacent line; `request_filter_impl.rs` and `callback.rs` additionally record `EdgionErrorCode::OutboundTlsSanMismatch` + `PluginLog` |
| JWKS SAN `warn!` | same counter |
| `"serving stale cache"` `warn!` | `discovery_jwks.rs::get_or_fetch_jwks` records `DyeOutcome::FailOpen("jwks-stale-cache")` → access log |
| `"JWKS refreshed successfully"` `info!` | nothing — a *successful* refresh is not an event worth a log line; it fires every `jwksCacheTtl` (default 300 s) per plugin instance forever |

The stale-cache warn is throttled (`expires_at` is bumped by `jwksMinRefreshInterval`, default 10 s)
but the two SAN warns are not: the discovery path has no equivalent backoff, so a misconfigured
`validation.subjectAltNames` plus a cold cache emits at request rate. That is exactly the
"rare because it needs a misconfiguration" framing rejected in the sibling findings.

**3. Relocating the one cold-valuable signal beat every structural option.**

`"OIDC discovery document loaded"` (issuer + jwks_uri) is the single site with real cold-path value
and no replacement channel — it confirms IdP reachability and the resolved endpoints. It fires at
most once per provider instance (the doc is cached permanently on success). Moving the emission to
`spawn_discovery_prewarm`, which already has the `Result`, preserves the signal verbatim at the cost
of a closure and two `if let Ok(doc)` bindings — **no signature change, no new API surface, no
control-flow change**. The three directions the todo floated (cold/hot entry-point split,
caller-kind flag, ban lazy fetching) all cost more and buy nothing extra.

Accepted consequence: a discovery load that happens *only* via the request-path lazy fetch logs
nothing. That case is already announced by the prewarm-exhausted `warn!`, whose text was updated in
the same change to stop pointing at the now-deleted "SAN pinning warn logs above".

**4. The gate could not have caught this, and adding the file alone would not have fixed that.**

`jwks_provider.rs` was outside `check_gateway_no_tracing.py`'s `DP_GLOBS` (which covers only
`plugins/http/*/plugin.rs`, `request_filter_impl.rs`, `request_stage.rs`), so the violation was
review-only. Adding it to `DP_FILES` was **not sufficient**: the scanner's `in_test` flag latched to
EOF on any `#[cfg(test)]` line, and this file has a `#[cfg(test)] use serde::Deserialize;` near the
top — every later hot-path `tracing::` would have been silently skipped, i.e. decorative coverage.

The latch now trips only on a test *module* declaration, `TEST_MOD_RE = \bmod\s+\w*tests?\b`. The
`\w*` prefix is load-bearing: several scanned files name their block `<topic>_tests`
(`authority_tests` in `pg_upstream_request_filter.rs`, `local_reply_tests` / `dye_strip_tests` in
`session_adapter.rs`), and a bare `mod tests?` would have un-latched them. A wholly test-only file
declared `#[cfg(test)] mod ...` by its parent has no such block at all, so
`traits/test_helpers.rs` is excluded by name via `NAME_SKIP` instead. Net effect measured against
the full 126-file scan set: zero new violations — the change is coverage-only, and it is verified by
injecting a canary `tracing::warn!` into `get_jwks` and confirming the gate exits non-zero.

## Fix Suggestions Not Accepted

- Re-add any `tracing::*` (including `debug!` or a separate `target:`) inside `jwks_provider.rs` —
  Iron rule 3 attaches to the path class, not the level. The file is now gate-enforced.
- Split `get_or_fetch_discovery` / `get_jwks` into cold and hot entry points — only one of the two
  is dual-use, and its cold caller can emit the log itself with no split (rationale 3).
- Thread a caller-kind enum or `is_cold: bool` down — it would have to propagate through
  `get_jwks` → `get_or_fetch_discovery` and every one of the five call sites to express one
  once-per-instance `info!`.
- Move lazy fetching off the request path so the provider is provably cold — prewarm retries are
  bounded (~5 min); after exhaustion the cache stays cold, so a request-path serve-or-fail
  provider would blackhole OIDC permanently on a slow-starting IdP.
- Add a `served_stale` / JWKS-refresh counter to replace the deleted lines — the stale serve already
  reaches the access log as a `FailOpen` dye outcome, and a successful-refresh counter has no
  consumer. (See the Iron rule 3 mechanisms table in
  [`../../03-coding/01-log-safety.md`](../../../03-coding/01-log-safety.md).)
- Keep the hardcoded `record_outbound_tls_san_mismatch("openid_connect")` — correct today only
  because `plugin.rs` is the sole `JwksProvider::new` caller; `component_label` exists precisely so
  a second consumer is not silently mislabelled.

## Re-evaluation Triggers

Re-open only if:

- A genuine background JWKS refresh task is introduced (today `get_jwks` has no cold caller), making
  `get_jwks` dual-use for the first time.
- `JwksProvider` gains a second consumer whose cold path needs a signal the prewarm task cannot
  emit.
- `edgion_gateway_outbound_tls_san_mismatch_total` is removed or stops carrying the `plugin` label.

## Reference Cases

- Origin: split out of EDG-GW-001; tracked as `tasks/todo/oidc-jwks-provider-dual-use-tracing.md`
  (deleted on close).
- Parent decision: [oidc-outbound-san-mismatch-tracing-removed.md](oidc-outbound-san-mismatch-tracing-removed.md).
- Files: `edgion-gateway/src/plugins/http/openid_connect/{jwks_provider.rs,plugin.rs}`,
  `cicd/checks/check_gateway_no_tracing.py`,
  `edgion-tests/integration/conf/EdgionPlugins/OpenidConnect/SanMismatch/README.md`.
- Iron rule 3: [`../../03-coding/01-log-safety.md`](../../../03-coding/01-log-safety.md).
- Sibling removals: [real-ip-run-request-tracing-removed.md](real-ip-run-request-tracing-removed.md),
  [wasm-request-path-tracing-removed.md](wasm-request-path-tracing-removed.md),
  [tls-route-plugin-access-log.md](../mandatory-design/tls-route-plugin-access-log.md),
  [dsl-vm-execute-tracing-declined.md](../mandatory-design/dsl-vm-execute-tracing-declined.md).

## Resolution references

- `oidc-jwks-provider-dual-use-tracing` — 2026-07-26.
