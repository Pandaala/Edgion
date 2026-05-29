---
name: dup-key-first-wins
description: Use when reviewing BasicAuth resolve_credential changes; documents the first-wins duplicate-username rule and the secret-data safety contract for password fields.
---

# BasicAuth — cross-group duplicate usernames are first-wins

The first-wins + warn policy (reject fail-fast / last-wins / silent-drop) is the
shared rule for the grouping series — see the canonical statement in
[key-auth/dup-key-first-wins.md](../key-auth/dup-key-first-wins.md). For BasicAuth
the duplicated identity is the **username**; the warn carries `secret_key`
(the "loser" pointer), `username`, `existing_group`, `incoming_group`.

Implementation:
`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/basic_auth.rs`

## Secret data safety iron rule (BasicAuth-specific)

`ResolvedBasicAuthUser.password: String` holds either plaintext or htpasswd-format
hash (`{SHA}<base64>` / `$apr1$...`). Any `tracing::*` in resolve_credential or the
gateway plugin must NOT carry the password value:

- Allowed: `username`, `secret_key`, `group`
- Forbidden: `password = %password`, `password_hash = %...`, any field that emits
  the password (literal or even hashed — the hash is still sensitive)

Reviewers must `grep "password\s*=\s*[%?]"` in `resolve_credential/basic_auth.rs`
and `gateway/plugins/http/basic_auth/` and confirm zero leak points.

## Related

- Canonical first-wins rule + sibling mirrors:
  [key-auth/dup-key-first-wins.md](../key-auth/dup-key-first-wins.md)
- `tasks/todo/plugin-grouping/05-basic-auth-spec.md` §3, §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
