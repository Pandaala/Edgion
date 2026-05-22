---
name: group-in-ok-log
description: Use when reviewing KeyAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> format and the rule that failure branches must NOT include group.
---

# KeyAuth — group name appears in OK access log, not in failures

After the KeyAuth `secret_groups` pilot, every successful key validation
writes the originating group into the access log:

- `OK group=<g> user <X>` — when `X-Consumer-Username` is present in the
  matched key's whitelisted headers.
- `OK group=<g>` — when no username header is configured.

Failure branches intentionally **do not** carry the group name:

- `Deny bad key`
- `Deny no key`
- `Allow anon`

## Threat model

- **Success line carries `group=`** — the caller already presented a valid
  key for that group, so they implicitly know which group they belong to.
  Surfacing it in PluginLog is operator information (group attribution for
  audit), not a credential leak.
- **Failure line MUST NOT carry `group=`** — an attacker who has not yet
  authenticated must not learn group topology by probing. If we logged
  `Deny bad key group=partner-a`, an attacker could brute-force key prefixes
  and watch which group surfaces, mapping out the multi-tenant layout.
- **Anonymous line MUST NOT carry `group=`** — anonymous bypasses key
  validation entirely; there is no group attribution to report.

Reviewers must reject patches that add `group=` to `Deny *` or `Allow anon`
lines.

Implementation lives in
`src/core/gateway/plugins/http/key_auth/plugin.rs` (the OK-log block in
`run_request`). Test fixture format in
`examples/code/client/suites/edgion_plugins/key_auth/key_auth.rs`.

Related: `skills/04-review/ip-restriction/group-name-in-deny-log.md`
documents the symmetric convention for IpRestriction (deny line carries
group; here, success line does — because the threat models differ).
