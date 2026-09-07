---
name: group-name-in-deny-log
description: Use when reviewing IpRestriction deny log findings; documents the group-name log format (deny:group=, deny:not-in-allow, deny:default) introduced in the 2026-05-14 refactor.
knowledge_type: mandatory-design
---

# IpRestriction — group name appears in deny logs

After the 2026-05-14 IP-list group refactor, IpRestriction deny logs
include the matched group name. When reviewing:

- `plugin_log` contains `"deny:group=<name>"` when a deny rule matches.
- `"deny:not-in-allow"` indicates whitelist-mode miss (allow configured
  but the IP did not match any allow group).
- `"deny:default"` / `"allow:default"` indicate the default action when
  no rule matched.
- `"deny:no-client-ip"` indicates the client IP could not be extracted.

For Stream plugins, the `StreamPluginResult::Deny(reason)` text is
`"ip:deny:group=<name>"` or `"ip:deny:not-in-allow"`.

Reviewers should confirm log assertions in test fixtures use the new
literal format, not the legacy `"Allow ip match"` / `"Deny ip match"`
strings.
