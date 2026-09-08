---
name: dup-key-first-wins
description: Use when reviewing HmacAuth resolve_credential changes; documents that cross-group duplicate usernames must use first-wins + warn (not fail-fast, not last-wins) and that warns must never carry the HMAC secret value.
knowledge_type: mandatory-design
---

# HmacAuth — cross-group duplicate usernames are first-wins

The first-wins + warn policy (reject fail-fast / last-wins / silent-drop) is the
shared rule for the grouping series — see the canonical statement in
[key-auth/dup-key-first-wins.md](../../key-auth/mandatory-design/dup-key-first-wins.md). For HmacAuth
the duplicated identity is the **username**; cross-group warnings must carry
`secret_key` plus `existing_group` and `incoming_group`, without raw identity
values. Within-group warnings use `secret_key` and `group`.

**Known implementation gap (D0036):** `hmac.rs::resolve_group` still emits raw
`username` in its short-secret warning and both duplicate-warning branches.
The previous username allowance here conflicted with canonical PII redaction;
it is withdrawn, not accepted as a tradeoff. The implementation remains deferred
in [the credential-copy audit task](../../../../tasks/todo/audit-auth-credential-copy-lifetimes.md).
Only BasicAuth warning fields are corrected in the current scoped change.

Implementation:
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac.rs`

## Secret data safety iron rule (HmacAuth-specific)

`HmacCredential.secret: Vec<u8>` is the HMAC shared key. Any `tracing::*` in this
file (first-wins, length warning, parse failure, future branches) must NOT carry
the secret value:

- Allowed: `secret_key`, `group`, `secret_len` (numeric), `min_len`
- Forbidden: raw username, `secret = %secret_value`, `secret_bytes = ?secret_value.as_bytes()`,
  any field that emits secret content (literal or hashed)

Reviewers must `grep secret =` in this file and confirm zero leak points.

## Related

- Canonical first-wins rule + sibling mirrors:
  [key-auth/dup-key-first-wins.md](../../key-auth/mandatory-design/dup-key-first-wins.md)
- `tasks/todo/plugin-grouping/02-hmac-auth-spec.md` §3 + §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
