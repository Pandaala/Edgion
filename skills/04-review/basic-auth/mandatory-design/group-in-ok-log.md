---
name: group-in-ok-log
description: Use when reviewing BasicAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> format and the rule that every failure branch must NOT include group.
knowledge_type: mandatory-design
---

# BasicAuth — group name appears in OK access log, not in failures

Per-plugin instance of the cross-feature [Access-log group attribution iron
rule](../../security/mandatory-design/iron-rules.md) — read it for the shared threat model
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
- `identity header rejected` (`push_err`) — the `X-Consumer-Username` /
  anonymous header write failed; the request is denied with the 401 so a
  client-supplied identity header never reaches the upstream. The headers are
  written **before** the `OK group=` or `Allow anonymous` line is pushed, so a
  denied request never carries a success line (since 2026-11-09). Every 401
  goes through one `deny` helper that applies `authFailureDelayMs`, so the
  authenticated header-rejection 401 is as slow as a bad-credential 401 and
  timing does not reveal the password check. Consequence (accepted
  fail-close): a configured username the header store rejects (a control
  character other than TAB) answers 401 permanently. An `anonymous` value that
  is not valid header text (`http::HeaderValue::from_str`) is rejected at
  admission, so it can no longer cause the permanent 401.
- `credential removal failed` (`push_err`) — with `hideCredentials`, the
  `Authorization` header could not be removed; the request is denied through
  the same delayed 401 instead of forwarding the credential. Both tags also
  record `EdgionErrorCode::AuthHeaderMutationRejected`.

## Implementation

- `edgion-gateway/src/plugins/http/basic_auth/plugin.rs` — the single
  `OK group={} user {}` `push_fmt` is the only line that carries `group=`; the
  `Deny no credential`, `Deny bad credential`, `Allow anonymous`,
  `identity header rejected` and `credential removal failed` calls carry no
  group/username. Unit tests in
  `edgion-gateway/tests/unit/plugins/http/basic_auth/plugin_test.rs` assert the
  OK line matches `OK group=...`, the Deny/Allow literals appear verbatim, and a
  rejected header write leaves no `OK group=` or `Allow anonymous` line. (Line numbers
  omitted — they drift; grep the literals.)

## Related

- Shared rule + sibling plugins (KeyAuth, HmacAuth, JwtAuth) and the inverse
  IpRestriction convention: [security/iron-rules.md](../../security/mandatory-design/iron-rules.md).
