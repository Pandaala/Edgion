---
name: dup-key-first-wins
description: Use when reviewing BasicAuth resolve_credential changes; documents the first-wins duplicate-username rule and the secret-data safety contract for password fields.
---

# BasicAuth — cross-group duplicate usernames are first-wins

The BasicAuth `secret_groups` feature (mirrored from KeyAuth/HmacAuth/JwtAuth
pilots) defines `secret_groups` as a management convenience, NOT a tenant
isolation boundary. When the same username appears in two groups, the
controller must:

1. **Keep the first occurrence** (iteration order of `secret_groups`)
2. **Drop the later occurrence**
3. **Emit `tracing::warn!`** with `secret_key` (the "loser" pointer),
   `username`, `existing_group`, `incoming_group` so operators can locate
   both Secrets

Implementation lives in:
`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/basic_auth.rs`

When reviewing changes in this file, reject:

- **fail-fast** (returning an error / rejecting the EdgionPlugins resource):
  one bad partner Secret would take down the whole plugin
- **last-wins**: ordering is HashMap-iteration-dependent and unpredictable
- **Silent drop**: the warn line is the only audit trail; do not remove it

## Secret data safety iron rule

`ResolvedBasicAuthUser.password: String` holds either plaintext or
htpasswd-format hash (`{SHA}<base64>` / `$apr1$...`). Any `tracing::*`
in resolve_credential or gateway plugin must NOT carry the password
value:

- Allowed: `username`, `secret_key`, `group`
- Forbidden: `password = %password`, `password_hash = %...`, any field
  that emits the password (literal or even hashed — the hash is still
  sensitive)

Reviewers must `grep "password\s*=\s*[%?]"` in
`resolve_credential/basic_auth.rs` and `gateway/plugins/http/basic_auth/`
and confirm zero leak points.

## Related

- `skills/04-review/key-auth/dup-key-first-wins.md` — same convention for KeyAuth
- `skills/04-review/hmac-auth/dup-key-first-wins.md` — same convention for HmacAuth
- `tasks/todo/plugin-grouping/05-basic-auth-spec.md` §3, §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
