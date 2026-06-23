---
name: group-in-ok-log
description: Use when reviewing BasicAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> format and the rule that every failure branch must NOT include group.
---

# BasicAuth — group name appears in OK access log, not in failures

Per-plugin instance of the cross-feature [Access-log group attribution iron
rule](../security/iron-rules.md) — read it for the shared threat model
(topology-probing oracle). BasicAuth specifics:

**OK line** — `OK group=<g> user <X>`, always (BasicAuth has no "username-less"
hit branch; the username comes from the Basic auth header and is always present
on the OK path).

**Failure / non-OK branches** (must NOT carry `group=` or the username):

- `Allow anonymous` — anonymous bypass, no group attribution
- `Deny no credential` — no Authorization header
- `Deny bad credential` — username not found OR password verification failed
  (single literal — by design, avoids the username-existence oracle; do NOT
  split into `Deny unknown user` vs `Deny wrong password`)

## Implementation

- `edgion-gateway/src/plugins/http/basic_auth/plugin.rs` — the single
  `OK group={} user {}` `push_fmt` is the only line that carries `group=`; the
  `Deny no credential`, `Deny bad credential`, and `Allow anonymous` `push` calls
  carry no group/username. An in-file test asserts the OK line matches
  `OK group=...` and the three Deny/Allow literals appear verbatim. (Line numbers
  omitted — they drift; grep the literals.)

## Related

- Shared rule + sibling plugins (KeyAuth, HmacAuth, JwtAuth) and the inverse
  IpRestriction convention: [security/iron-rules.md](../security/iron-rules.md).
