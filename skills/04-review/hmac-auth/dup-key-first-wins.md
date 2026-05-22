---
name: dup-key-first-wins
description: Use when reviewing HmacAuth resolve_credential changes; documents that cross-group duplicate usernames must use first-wins + warn (not fail-fast, not last-wins) and that warns must never carry the HMAC secret value.
---

# HmacAuth — cross-group duplicate usernames are first-wins

The HmacAuth `secret_groups` feature (mirrored from the KeyAuth pilot)
defines `secret_groups` as a management convenience, NOT a tenant isolation
boundary. When the same username appears in two groups, the controller
must:

1. **Keep the first occurrence** (iteration order of `secret_groups`),
2. **Drop the later occurrence**,
3. **Emit a `tracing::warn!`** that carries `secret_key` + `username` + both
   `existing_group` and `incoming_group` fields so operators can locate
   both Secrets.

Implementation lives in:
`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac.rs`

When reviewing changes in this file, reject:

- **fail-fast** (returning an error / rejecting the EdgionPlugins resource):
  one bad partner Secret would take down the whole plugin.
- **last-wins**: ordering is HashMap-iteration-dependent and unpredictable.
- **Silent drop**: the warn line is the only audit trail; do not remove it.

## Secret data safety iron rule

`HmacCredential.secret: Vec<u8>` is the HMAC shared key. Any `tracing::*`
in this file (first-wins, length warning, parse failure, future branches)
must NOT carry the secret value:

- Allowed: `username`, `secret_key`, `group`, `secret_len` (numeric), `min_len`
- Forbidden: `secret = %secret_value`, `secret_bytes = ?secret_value.as_bytes()`,
  any field that emits secret content (literal or hashed)

Reviewers must `grep secret =` in this file and confirm zero leak points.

## Related

- `skills/04-review/key-auth/dup-key-first-wins.md` — same convention for KeyAuth
- `tasks/todo/plugin-grouping/02-hmac-auth-spec.md` §3 + §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
