---
name: dup-key-first-wins
description: Use when reviewing HmacAuth resolve_credential changes; documents that cross-group duplicate usernames must use first-wins + warn (not fail-fast, not last-wins) and that warns must never carry the HMAC secret value.
---

# HmacAuth — cross-group duplicate usernames are first-wins

The first-wins + warn policy (reject fail-fast / last-wins / silent-drop) is the
shared rule for the grouping series — see the canonical statement in
[key-auth/dup-key-first-wins.md](../key-auth/dup-key-first-wins.md). For HmacAuth
the duplicated identity is the **username**; the warn carries `secret_key` +
`username` + both `existing_group` and `incoming_group`.

Implementation:
`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac.rs`

## Secret data safety iron rule (HmacAuth-specific)

`HmacCredential.secret: Vec<u8>` is the HMAC shared key. Any `tracing::*` in this
file (first-wins, length warning, parse failure, future branches) must NOT carry
the secret value:

- Allowed: `username`, `secret_key`, `group`, `secret_len` (numeric), `min_len`
- Forbidden: `secret = %secret_value`, `secret_bytes = ?secret_value.as_bytes()`,
  any field that emits secret content (literal or hashed)

Reviewers must `grep secret =` in this file and confirm zero leak points.

## Related

- Canonical first-wins rule + sibling mirrors:
  [key-auth/dup-key-first-wins.md](../key-auth/dup-key-first-wins.md)
- `tasks/todo/plugin-grouping/02-hmac-auth-spec.md` §3 + §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
