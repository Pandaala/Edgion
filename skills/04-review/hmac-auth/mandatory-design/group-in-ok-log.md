---
name: group-in-ok-log
description: Use when reviewing HmacAuth PluginLog assertions or fixture changes; documents the OK-line group= format and the rule that every failure branch must NOT include group.
knowledge_type: mandatory-design
---

# HmacAuth — group name appears in OK access log, not in failures

Per-plugin instance of the cross-feature [Access-log group attribution iron
rule](../../security/mandatory-design/iron-rules.md) — read it for the shared threat model
(topology-probing oracle). HmacAuth specifics:

**OK line** — `OK group=<g> user <X>`, always (HmacAuth has no "username-less"
hit branch: `params.username` is a mandatory Authorization-header field, so the
format is always 2 fields).

**Failure branches** (must NOT carry `group=`):

- `Deny missing auth`
- `Deny bad format`
- `Deny bad algorithm`
- `Deny missing key` — especially critical: a guessed keyId could be used to
  probe group topology if `group=` were leaked here.
- `Deny missing date signed`
- `Deny bad timestamp`
- `Deny missing required header`
- `Deny missing header`
- `Deny bad signature`
- `Allow anonymous` (anonymous has no group attribution)

Internal error codes (`no creds configured`) are already non-Deny `push_err`
calls and are unrelated to group attribution — do not add `group=` to them.

## Implementation

- `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs` — the OK-log block in
  `run_request` (grep `OK group=`; line numbers drift).

## Related

- Shared rule + sibling plugins, and the inverse IpRestriction convention
  ([ip-restriction/group-name-in-deny-log.md](../../ip-restriction/mandatory-design/group-name-in-deny-log.md),
  where the deny line *does* carry group): [security/iron-rules.md](../../security/mandatory-design/iron-rules.md).
