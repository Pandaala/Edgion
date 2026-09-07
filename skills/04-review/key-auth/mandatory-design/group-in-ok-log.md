---
name: group-in-ok-log
description: Use when reviewing KeyAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> format and the rule that failure branches must NOT include group.
knowledge_type: mandatory-design
---

# KeyAuth — group name appears in OK access log, not in failures

Per-plugin instance of the cross-feature [Access-log group attribution iron
rule](../../security/mandatory-design/iron-rules.md) — read it for the shared threat model
(topology-probing oracle). KeyAuth specifics:

**OK line**:

- `OK group=<g> user <X>` — when `X-Consumer-Username` is present in the matched
  key's whitelisted headers.
- `OK group=<g>` — when no username header is configured.

**Failure branches** (must NOT carry `group=`): `Deny bad key`, `Deny no key`,
`Allow anon`. (`Deny bad key group=partner-a` would let an attacker brute-force
key prefixes and watch which group surfaces.)

## Implementation

- `edgion-gateway/src/plugins/http/key_auth/plugin.rs` — the OK-log block in
  `run_request`. Test fixture format in
  `edgion-tests/src/client/suites/edgion_plugins/key_auth/key_auth.rs`.

## Related

- Shared rule + sibling plugins, and the inverse IpRestriction convention
  ([ip-restriction/group-name-in-deny-log.md](../../ip-restriction/mandatory-design/group-name-in-deny-log.md),
  where the deny line *does* carry group): [security/iron-rules.md](../../security/mandatory-design/iron-rules.md).
