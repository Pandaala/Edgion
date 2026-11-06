# res-04: Reserved/unwired fields are published as operator API (upstreamResponsePlugins, ConfigData `active`/`Misc`, LDAP `keepalive`)

- **Category**: over-design
- **Severity**: P3 — downgraded from P2 by independent verification
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/mod.rs:129` (`upstream_response_plugins`), `entry.rs:466-540` (`UpstreamResponseEntry`, "Reserved for future use"; reserved `body`)
  - `edgion-resources/src/resources/edgion_plugins/validate.rs:99-133` (no plugin type is eligible for that stage)
  - `config/crd/edgion-crd/edgion_plugins_crd.yaml:669` (`upstreamResponsePlugins` schema)
  - `edgion-resources/src/resources/refs/condition_fields.rs:1356-1376`, `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:2128` (walkers/runtime builder for the empty stage)
  - `edgion-resources/src/resources/edgion_config_data/mod.rs:105` (`spec.active`, "Not yet wired"), `:262` (`ConfigEntry::Misc`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/ldap_auth.rs:98` (`keepalive`, "Reserved")
  - `edgion-resources/src/resources/edgion_plugins/entry.rs:102` (`PluginEntry`, re-exported but unused by production code)

## Summary

Several CRD fields are accepted, schema-documented, synced and walked by reference/condition code, yet have
no behavior. The largest is a whole plugin stage: `EdgionPlugins.spec.upstreamResponsePlugins` accepts a
list of entries, but `applicable_stages_by_name` returns `false` for that stage for every plugin type, so any
entry placed there fails validation. Others are envelope fields with no consumer. The project is
pre-release with an explicit "no reserved/compat surface" rule; these fields cost schema, docs, walkers and
review effort and mislead operators.

## Current behavior (evidence)

- `HTTP_PLUGIN_STAGE_EXCEPTIONS` lists the only non-request plugins (ResponseHeaderModifier,
  DebugAccessLogToHeader, ResponseRewrite, BandwidthLimit, Dsl, Wasm); the fourth `StageSet` column is
  `false` for all of them, and the default is `REQUEST_ONLY`. The doc comment says "The async
  upstream-response stage currently has no directly configurable plugin."
- Yet the stage is a full CRD field with its own entry type (`UpstreamResponseEntry` with `enable`,
  `conditions`, `alias`, reserved `body` that "validation rejects ... for every currently supported
  response-stage plugin", `dye`), condition reference walkers (`condition_fields.rs:1356-1376`), a Gateway
  builder (`add_from_upstream_responses`), and user docs (`plugin-composition.md`: "Reserved async response
  hook (no built-in plugins map here today)").
- `EdgionConfigData.spec.active`: doc comment "Not yet wired"; the only Controller uses are checks that it is
  *absent* for strict variants (`handlers/gateway.rs:381`, `handlers/edgion_plugins.rs:664,716,807`). The
  `Selector` variant carries its own `active`, so there are two `active` fields with different meanings.
- `ConfigEntry::Misc(serde_json::Value)`: an unvalidated, preserve-unknown payload. The Gateway's ConfigData
  handler explicitly builds no index for it (`edgion-gateway/.../edgion_config_data/conf_handler.rs:268`) and no
  plugin reads it. It also undermines the "unknown extension type is invalid" model (`Unsupported`) by
  offering a sanctioned bucket for arbitrary, unbounded data that is broadcast to every gateway.
- `LdapAuthConfig.keepalive` ("Reserved keepalive setting"): never read in `edgion-gateway/src/plugins/http/ldap_auth/`.

## Industry comparison

Kubernetes API conventions and Gateway API GEP process: fields are added when implemented (behind an
experimental channel if needed), never as inert placeholders. Envoy marks unimplemented proto fields
`[#not-implemented-hide:]` and rejects them at config load.

## Impact

Operators can author configuration that silently does nothing (`active`, `keepalive`, `Misc`) or always fails
(`upstreamResponsePlugins`). Every plugin-wide mechanism (condition walkers, dye, alias, policy masking,
reference binding) must carry a fifth/fourth arm for an empty stage, and every new plugin must be classified
against it.

## Suggested direction

Delete, per the pre-release no-compat rule: `upstreamResponsePlugins` + `UpstreamResponseEntry` + its runtime
builder/walkers/CRD/doc rows; `EdgionConfigData.spec.active` (keep `Selector.active`); `ConfigEntry::Misc`
(an unknown type already decodes to `Unsupported`); `LdapAuthConfig.keepalive`; unused `PluginEntry`. Re-add
an async response stage only together with its first plugin.

## Notes

- No intentional-design page covers these fields. `edgion-config-data-managed-by-is-inert.md` concerns a
  different inert item (the `edgion.io/managed-by` label, wont-fix for an ownership mechanism); it does not
  decide whether inert spec fields should remain in the CRD.
- `tasks/todo/http-plugin-wiring-plan/` counts stage wiring sites per plugin; removing the empty stage reduces
  that count but is not part of that plan.

## Verification (independent)

- **Verdict**: DOWNGRADED — P2 → P3. The claims are accurate, but the impact is API hygiene. `upstreamResponsePlugins` fails validation loudly rather than silently, and the remaining inert fields (`active`, `keepalive`, `Misc`) are minor no-ops. The cleanup is worth doing before the API freezes, but it has no runtime consequence.
- `edgion_plugins/validate.rs:99-112,119-133`: no `HTTP_PLUGIN_STAGE_EXCEPTIONS` row sets the fourth (`upstream_response`) column, and the default is `REQUEST_ONLY`.
- `edgion_config_data/mod.rs:100-105` (`active`: "Not yet wired") and `:258-262` (`Misc`). The only Gateway references are `edgion_config_data/conf_handler.rs:268,375`, and neither builds an index.
- `ldap_auth.rs:96-98` `keepalive` ("Reserved") is never read under `edgion-gateway/src/plugins/http/ldap_auth/`. No review page records a decision to keep these placeholders.

## Additional evidence (wave 3)

The same "installed seam, no consumer" pattern exists in Gateway internals:
`edgion-gateway/src/request_body/observer.rs` (412 lines: `RequestBodyObserver` trait, sealed
marker, budgets, panic-isolated slots, per-attempt reset, log transfer) is compiled with
`#![allow(dead_code, reason = "WP5 installs the seam; WP6 adds its first production observer")]`.
There is no production implementation of `RequestBodyObserver`, and the only production
`install_request_body_observer` (`plugins/runtime/session_adapter.rs:2476`) has no caller. The
seam is nevertheless wired into the hot request-body path
(`routes/http/proxy_http/pg_request_body_filter_action.rs:139, 395`) and into response-cache
eligibility (`response_cache/lifecycle.rs:40`). Under the pre-release "no reserved surface" rule it
should be removed until its first consumer lands (internal, so lower priority than the CRD fields
above).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `edgion_plugins/mod.rs`, `entry.rs`, `validate.rs`, `edgion_config_data/mod.rs`, `ldap_auth.rs` and `edgion_plugins_crd.yaml` are unchanged; `upstreamResponsePlugins`, `spec.active`, `ConfigEntry::Misc` and `keepalive` remain inert.
- `plugin_runtime.rs`/`session_adapter.rs` changed by one line each (no line shift); `request_body/observer.rs` is unchanged and still has no production implementation.
