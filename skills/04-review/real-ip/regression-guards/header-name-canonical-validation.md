---
name: header-name-canonical-validation
description: Use when reviewing RealIp `realIpHeader` findings; covers the canonical token/length admission check shared by the global `spec.realIp` and the RealIp plugin, and why a runtime filter or a reserved-name list is not accepted.
status: fixed
finding_id: validate-real-ip-header-names
closed: 2026-09-06
knowledge_type: regression-guards
---

# RealIp `realIpHeader` canonical name validation — FIXED

## Conclusion

`RealIpConfig.real_ip_header` is validated once at the resources admission
boundary with the canonical HTTP field-name seam
(`gwapi_types::is_valid_header_token` + `HEADER_NAME_MAX_LEN`). Both consumers
share one validation body (`RealIpConfig::check`): the RealIp plugin through
`PluginValidation::validation_error`, and the global
`EdgionGatewayConfig.spec.realIp` through
`EdgionGatewayConfigSpec::validate_real_ip`, wired into the Controller's
`EdgionGatewayConfigHandler::preparse`.

Fix suggestions of the form "filter or normalize the header name on the request
path", "add a RealIp-local header-name parser", or "reuse
`validate_remote_ip_header_name` so reserved names are rejected too" are **not
accepted**.

## Core Rationale

**1. The defect was silent inertness, not a runtime bug**

Before this fix the only check was `real_ip_header.trim().is_empty()`. A
non-token name such as `"X Forwarded For"` was published to the data plane,
where `HeaderMap::get` (`real_ip_extractor.rs`) and `session.header_str`
(`plugins/http/real_ip/plugin.rs`) correctly return `None` for it forever.
Extraction became permanently inert and `RemoteIp` stayed pinned to the trusted
proxy address, with no error, no `Accepted=False`, and no startup failure —
which silently skews access-log identity and every `RemoteIp`-based IP ACL.

**2. Admission is the fix location, with a Gateway backstop because preparse only warns**

The runtime lookup is correct HTTP behavior; adding a hot-path filter would keep
the invalid configuration accepted and merely move the symptom.

Parity comes from both sides running the same `edgion-resources` code, but the
two consumers reach it differently, and this distinction must not be blurred:

- **Plugin path** — `EdgionPlugin::validation_error` feeds `seal_fail_closed`,
  so an invalid RealIp plugin config prepends `RejectFilter` and every matching
  request gets `500 plugin configuration error`. This is a real behavior change
  from the previous silent no-op and is documented in
  `docs/{en,zh-CN}/user-guide/edgion-plugins/real-ip.md`.
- **Global path** — Controller preparse follows the repository's
  warn-not-reject policy: an `Accepted=False/reason=Invalid` condition is
  stamped and the resource is still synced. So `validate_real_ip` alone does not
  keep an invalid `spec.realIp` out of the data plane. The Gateway therefore
  re-runs it in `runtime/server/listener_builder.rs` and refuses to build the
  extractor, mirroring the existing `forwardedHeaders` backstop in
  `config/edgion_gateway/conf_handler_impl.rs`. On the Kubernetes path the CRD's
  `pattern`/`maxLength` on `realIpHeader` rejects the object at the API server
  before any of this runs.

**3. Read direction, so no reserved-name list**

`validate_remote_ip_header_name` guards the *write* direction
(`forwardedHeaders.remoteIpHeader`) and rejects framing, hop-by-hop and
`X-Forwarded-For` names. RealIp is the *read* direction, where
`X-Forwarded-For` is the default and must stay valid. Only the token charset and
the 256-byte bound are shared.

## Fix Suggestions Not Accepted

- Filtering or normalizing the name on the request path — leaves invalid configuration accepted and adds hot-path work.
- A RealIp-local field-name parser — duplicates `is_valid_header_token`, which is kept in lockstep with `HEADER_NAME_PATTERN` by `gwapi_types::tests::header_token_fn_matches_pattern`.
- Reusing `validate_remote_ip_header_name` — its reserved-name list would reject the RealIp default `X-Forwarded-For`.
- Echoing the rejected header name in the error string — the message is kept fixed and non-sensitive.

## Re-evaluation Triggers

Re-open only if:

- A consumer needs to read a header name outside the RFC 9110 `token` charset (no such transport exists today).
- The canonical seam moves out of `gwapi_types`, in which case `RealIpConfig::check` follows it rather than growing a local copy.

## Reference Cases

- Task `tasks/todo/validate-real-ip-header-names.md` (deferred from B0098), closed 2026-09-06.
- Implementation: `edgion-resources/src/resources/edgion_plugins/plugin_configs/real_ip.rs`, `edgion-resources/src/resources/edgion_gateway_config.rs`, `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs`, `edgion-gateway/src/runtime/server/listener_builder.rs`, `config/crd/edgion-crd/edgion_gateway_config_crd.yaml`.
- Side effect recorded on purpose: `validate_real_ip` runs the whole `check` body, so `spec.realIp` is now also rejected for an empty `trustedIps`, a malformed or duplicated CIDR, and an out-of-charset or duplicate `IpGroup` name — shapes `RealIpExtractor::new` used to accept and degrade silently.
- Tests: `test_plugin_validation_rejects_non_token_header_names`, `test_plugin_validation_rejects_oversized_header_name`, `test_plugin_validation_accepts_default_and_custom_header_names`, `egc_invalid_real_ip_header_is_rejected_at_ingest`.
- Sibling rule: [`max-trusted-hops-opt-in.md`](../intentional-design/max-trusted-hops-opt-in.md).

## Resolution references

- `task-validate-real-ip-header-names` — 2026-09-06.
