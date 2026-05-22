---
name: group-in-ok-log
description: Use when reviewing BasicAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> format and the rule that every failure branch must NOT include group.
---

# BasicAuth — group name appears in OK access log, not in failures

After the BasicAuth `secret_groups` rollout, successful authentication
writes:

- `OK group=<g> user <X>` — always (BasicAuth has no "username-less" hit
  branch; the username comes from the Basic auth header and is always
  present on the OK path)

Failure / non-OK branches intentionally **do not** carry the group name:

- `Allow anonymous` — anonymous bypass, no group attribution
- `Deny no credential` — no Authorization header
- `Deny bad credential` — username not found OR password verification
  failed (cannot distinguish from log message — by design, avoids
  username-existence oracle)

## Threat model

- **Success line carries `group=<g>`** — caller already authenticated;
  surfacing group attribution in PluginLog is operator audit info
- **Failure line MUST NOT carry group/username** beyond what `Deny bad credential`
  already implies. Critically, **never include** the username on the
  failure line (lookups for unknown users return the same `Deny bad
  credential` literal — preserves the "no username-existence oracle"
  property)

Reviewers must reject patches that:

- Add `group=` to `Deny *` or `Allow anonymous` lines
- Split `Deny bad credential` into `Deny unknown user` vs `Deny wrong password`
  (would create the oracle)

## Implementation

Verify code lives in:
- `src/core/gateway/plugins/http/basic_auth/plugin.rs` — OK log push around
  line 273; failure push at lines 221, 254, 265

## Related

- `skills/04-review/key-auth/group-in-ok-log.md` — same convention for KeyAuth
- `skills/04-review/hmac-auth/group-in-ok-log.md` — same convention for HmacAuth
- `skills/04-review/jwt-auth/group-in-ok-log.md` — same convention for JwtAuth
