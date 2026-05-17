---
name: group-in-ok-log
description: Use when reviewing HmacAuth PluginLog assertions or fixture changes; documents the OK-line group= format and the rule that every failure branch must NOT include group.
---

# HmacAuth — group name appears in OK access log, not in failures

After the HmacAuth `secret_groups` mirror of the KeyAuth pilot, every
successful HMAC validation writes the originating group into the access log:

- `OK group=<g> user <X>` — always (HmacAuth has no "username-less" hit
  branch: `params.username` is a mandatory Authorization-header field, so
  the format is always 2 fields).

Failure branches intentionally **do not** carry the group name:

- `Deny missing auth`
- `Deny bad format`
- `Deny bad algorithm`
- `Deny missing key`  — especially critical: a guessed keyId could be used
  to probe group topology if `group=` were leaked here.
- `Deny missing date signed`
- `Deny bad timestamp`
- `Deny missing required header`
- `Deny missing header`
- `Deny bad signature`
- `Allow anonymous` (anonymous has no group attribution)

Internal error codes (`body not supported`, `no creds configured`) are
already non-Deny `push_err` calls and are unrelated to group attribution
— do not add `group=` to them.

## Threat model

- **Success line carries `group=`** — the caller already authenticated
  for that group, so they implicitly know their attribution. Surfacing
  it in PluginLog is operator information for audit.
- **Failure line MUST NOT carry `group=`** — an unauthenticated attacker
  must not be able to probe which group a guessed username belongs to.
  Surfacing it on `Deny missing key` would map out tenant topology
  under brute force.

Reviewers must reject patches that add `group=` to any `Deny *` or
`Allow anonymous` line.

Implementation lives in
`src/core/gateway/plugins/http/hmac_auth/plugin.rs` (the OK-log block in
`run_request`, around line 482).

## Related

- `skills/04-review/key-auth/group-in-ok-log.md` — same convention for KeyAuth
- `skills/04-review/ip-restriction/group-name-in-deny-log.md` — symmetric
  convention for IpRestriction (different threat model: IpRestriction deny
  line DOES carry group because the attacker model is different there).
