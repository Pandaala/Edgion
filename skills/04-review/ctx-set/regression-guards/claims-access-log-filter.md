---
name: claims-access-log-filter
description: Read when reviewing store_claims_in_ctx / store_payload_in_ctx flowing JWT/JWE claims unfiltered into the access log / jwt_claims / oidc_claims / jwe_payload keys reaching access-log JSON via ctx_map (gateway-plugins-03).
knowledge_type: regression-guards
---

# gateway-plugins-03 — JWT/JWE claims unfiltered into access log

**Status / verdict**: fixed (gateway-plugins-03, 2026-05-25; blocklist `FilteredCtx` lost from the tree and re-landed 2026-06-14; root-cause hardened 2026-06-21 with a deny-by-default per-route allowlist; **superseded again 2026-07-16** — the per-route allowlist annotation was itself deleted in favor of an explicit `AccessLogExtern` declaration).

When reviewing findings of the form "`store_claims_in_ctx` / `store_payload_in_ctx` flow JWT/JWE claims unfiltered into the access log / `jwt_claims` / `oidc_claims` / `jwe_payload` keys reach access-log JSON via `ctx_map` / no allowlist or masking on `ctx_map` claim keys (gateway-plugins-03)": fixed.

**Current design (2026-07-16): explicit declaration, hard-blocked at the source-type level.** A `ctx_map` value only reaches the log when an operator writes an `AccessLogExtern` entry naming it — `{key: <field>, from: ctx, name: <ctx_map key>}` on `EdgionPlugins.spec.accessLogExtern` — and that declaration is resolved only at `logging()` (never inside a request-stage filter). This superseded the 2026-06-21 `AllowlistedCtx` design (a per-route ctx-key allowlist annotation, plus a separate unified JSON log-projection annotation), both of which are now deleted entirely; there is no compatibility shim.

**Validity limit:** this guard covers direct declarations of the canonical keys,
not sensitive values copied into other ctx names. The configuration-dependent
alias exposure remains a real defect accepted without a runtime change; see
[sensitive ctx aliases](../../observability/intentional-design/ctx-provenance-unmasking-risk.md)
for the user decision, source evidence, and re-evaluation triggers.

The `jwt_claims` / `oidc_claims` / `jwe_payload` blocklist survives as `ACCESS_LOG_BLOCKED_CTX_KEYS`
(`edgion-resources/src/resources/edgion_plugins/plugin_configs/access_log_extern.rs`) and enforcement
is two-part, not a single ingest gate — **admission in this repo is lenient**: a preparse error is
reported on the resource's status, but the resource still lands in the cache conf_sync serves to the
gateway.

- **Reported at `EdgionPlugins` ingest**: an `AccessLogExtern` entry naming a blocked `ctx` key
  produces a preparse error surfaced on the resource's `Accepted=False` status condition. This is
  the operator-facing signal — it does **not** keep the resource off the cluster; the entry is
  still present in the spec the controller caches and syncs.
- **Dropped at gateway load**: the actual enforcement is `compile_plugins`
  (`edgion-gateway/src/plugins/http/plugin_store.rs`), which `retain()`s the declared fields and
  drops any whose `validation_error()` is `Some` when the gateway loads the resource. A blocked
  `ctx` key never reaches `ext` because it is never in the compiled `LogExt` field list — not
  because it was kept off the cluster.

Downstream plugins still read these keys via `session.get_ctx_var` (in-memory, per request); only
the log projection is gated. Any new canonical `ctx_map` key that stores claims, decrypted
payloads, tokens, or other secrets MUST be added to `ACCESS_LOG_BLOCKED_CTX_KEYS`, as required by
[design rule A](../../../00-design-rules.md#a-sensitive-data--log-redaction). It stays a hard,
un-whitelistable block regardless of the EGC masking whitelist. The separately accepted
configuration-dependent alias risk does not weaken this requirement for new canonical keys.

Full design: `docs/superpowers/specs/2026-07-16-access-log-extern-plugin-design.md`. Unit coverage:
`blocked_ctx_keys_are_rejected` in `access_log_extern.rs`;
`access_log_extern_blocked_header_is_reported_at_ingest` in
`edgion-resources/tests/unit/resources/edgion_plugins/mod_test.rs` (reports on `Accepted=False`; does not
keep the resource off the cluster); `compile_plugins` drop coverage in
`edgion-gateway/src/plugins/http/plugin_store.rs` (the actual enforcement). To confirm the blocklist
holds, check both halves: `is_blocked(LogExtSource::Ctx, ...)` (the shared list the report and the
drop both consult) and the `compile_plugins` `retain()` (where the drop actually happens) —
`validate_plugins`/ingest reporting alone does not prove the field is kept out of the log; the
`retain()` is what does.
