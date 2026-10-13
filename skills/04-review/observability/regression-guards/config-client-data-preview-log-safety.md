---
name: config-client-data-preview-log-safety
description: Use when reviewing log-safety findings of the form "ConfigClient::apply_resource_change logs data_preview = &data[..N], a body slice that can leak TLS key / LinkSys credential bytes" — already fixed to data_len.
status: fixed
finding_id: conf-sync-F005
closed: 2026-05-28
knowledge_type: regression-guards
---

# ConfigClient apply_resource_change data_preview Log Leak — FIXED (Iron Rule 1)

## Conclusion

`ConfigClient::apply_resource_change` (`edgion-gateway/src/conf_sync/conf_client/config_client.rs:582-665`, `impl ConfigClientEventDispatcher`) emitted `data_preview = &data[..floor_char_boundary(200)]` on all three error paths. Because `EdgionTls` and `LinkSys` are dispatched through the same `apply_change!` macro, a parse failure on a TLS cert/key body or a LinkSys credential body could dump up to the first 200 bytes of secret material into the ConfHandler error log — a direct breach of Iron Rule 1. Fixed by replacing `data_preview` with `data_len = data.len()` at all three sites.

Re-reports of the `data_preview = &data[..N]` form on this function are **not accepted**; verify the three `tracing` sites carry `data_len = data.len()` (not a body slice) before re-reporting.

## Core Rationale

**1. The three leak sites**

The "Failed to determine resource type" `tracing::error!` (L594), the `log_error` closure shared by the `apply_change!` macro on every `serde_yaml::from_str` failure (L606), and the "Unspecified resource kind, skipping" `tracing::warn!` (L626) all carried the body slice.

**2. Why it breaches Iron Rule 1**

`EdgionTls` (L644) and `LinkSys` (L640) are dispatched through the same `apply_change!` macro, so a parse failure on a TLS cert/key body or a LinkSys credential body would dump the first 200 bytes of secret material into the error log — a direct breach of `skills/03-coding/01-log-safety.md` Iron Rule 1 ("Password / certificate private key … Do not record at all; only record the operation result").

**3. Reachability today is zero, but fixed proactively**

A codebase-wide grep finds only the trait declaration (`common/conf_sync/traits.rs:79`) and this impl, with no callers and no `dyn`/`Box<dyn ConfigClientEventDispatcher>` dispatch — the live gRPC sync path routes through `cache_client/event_dispatch.rs:53`, which already logs only `kind`/`name`/`namespace`/`version`. Latent High (would-be-Critical on Iron Rule 1 the moment the dispatcher is wired up); fixed proactively so the live-cache and config-client error-log surfaces stay symmetric.

**4. The fix**

Replaced `data_preview` with `data_len = data.len()` (byte length) at all three sites; the existing `error = %err` attribute was retained. That historical change removed explicit body previews only: `serde_yaml::Error` Display can itself contain authored values or key paths, so it is not a value-free diagnostic boundary.

## Fix Suggestions Not Accepted

- Truncating the preview to fewer bytes — any body slice on a TLS/LinkSys path is a leak; byte length is the correct shape.

## Re-evaluation Triggers

Re-evaluate if:

- A new error path on this function reintroduces a body slice into a `tracing` attribute.
- A reachable parser diagnostic demonstrates authored-value exposure, even without a body preview.

## Reference Cases

- Source: 2026-05-28 conf-sync log-safety review (conf-sync-F005).
- Iron Rule: `skills/03-coding/01-log-safety.md` Iron Rule 1.
- Precedent: `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:53`.

## Historical resolution evidence: conf-sync-F005

`ConfigClient::apply_resource_change` (`edgion-gateway/src/conf_sync/conf_client/config_client.rs:582-665`, `impl ConfigClientEventDispatcher`) emitted `data_preview = &data[..floor_char_boundary(200)]` on all three error paths — the "Failed to determine resource type" `tracing::error!` (L594), the `log_error` closure shared by the `apply_change!` macro on every `serde_yaml::from_str` failure (L606), and the "Unspecified resource kind, skipping" `tracing::warn!` (L626). `EdgionTls` (L644) and `LinkSys` (L640) are dispatched through the same `apply_change!` macro, so a parse failure on a TLS cert/key body or a LinkSys credential body would dump up to the first 200 bytes of secret material into the ConfHandler error log — a direct breach of `skills/03-coding/01-log-safety.md` Iron Rule 1 ("Password / certificate private key … Do not record at all; only record the operation result"). Reachability today is **zero**: a codebase-wide grep finds only the trait declaration (`common/conf_sync/traits.rs:79`) and this impl, with no callers and no `dyn`/`Box<dyn ConfigClientEventDispatcher>` dispatch — the live gRPC sync path routes through `cache_client/event_dispatch.rs:53`, which already logs only `kind`/`name`/`namespace`/`version`. Fixed by replacing `data_preview` with `data_len = data.len()` (byte length) at all three sites; the existing `error = %err` attribute was retained. That historical change removed explicit body previews only: `serde_yaml::Error` Display can itself contain authored values or key paths, so it is not a value-free diagnostic boundary. Latent High (would-be-Critical on Iron Rule 1 the moment the dispatcher is wired up); fixed proactively so the live-cache and config-client error-log surfaces stay symmetric without establishing safety of every parser diagnostic in `conf_sync/`. Re-reports of the `data_preview = &data[..N]` form on this function are not accepted; verify the three `tracing` sites carry `data_len = data.len()` (not a body slice) before re-reporting.

## Resolution references

- `conf-sync-F005` — 2026-05-28.


## Admin parser diagnostic boundary (ARCH-140)

`edgion-controller/src/api/common.rs` keeps the existing two WARN events for
resource parsing and source-version rejection, but records only a fixed `stage`
(`resource_parse` or `source_version`) and numeric `line`/`column` when available.
Neither error Display nor authored values/key paths enter these events. Custom
source-version errors may have no location; coordinates are then absent.
JSON-first/YAML fallback, API-version normalization, HTTP 400 mapping and admission
ordering are unchanged.

`tests/unit/api/common_test.rs` captures actual local-subscriber events for JSON
and YAML unknown TLS modes, malformed scalar values, a dynamic map key, malformed
source syntax and unsupported versions; valid documents remain accepted.
`namespaced_handlers_test/update_identity_tests.rs` verifies a rejected TLS update
leaves the existing filesystem resource unchanged. These are in-process tests,
not a network/authentication test. ConfigClient's remaining error diagnostics are
outside this correction and gain no safety endorsement from the historical
preview removal.
