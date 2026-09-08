---
name: dup-key-first-wins
description: Use when reviewing KeyAuth resolve_credential changes; canonical statement of the first-wins + warn rule for cross-group duplicate identities across the grouping plugins (KeyAuth pilot; BasicAuth/HmacAuth mirror it).
knowledge_type: mandatory-design
---

# Cross-group duplicate identities are first-wins (KeyAuth pilot — canonical)

> This is the canonical statement of the duplicate-identity rule for the whole
> plugin-grouping series. KeyAuth was the pilot; BasicAuth / HmacAuth mirror it
> and link here for the policy (they add only their own secret-safety tail).

`secret_groups` (KeyAuth/BasicAuth/HmacAuth) is a **management convenience, NOT a
tenant isolation boundary**. When the same identity (raw API key for KeyAuth;
username for Basic/Hmac) appears in two groups, the controller must:

1. **Keep the first occurrence** (iteration order of `secret_groups`),
2. **Drop the later occurrence**,
3. **Emit a `tracing::warn!`** carrying both `existing_group` and `incoming_group`
   plus `secret_key` so operators can locate the rejected Secret and the relevant
   groups. Within-group warnings carry `secret_key` and `group`. Do not emit raw
   identity values; resource pointers are sufficient for follow-up.

The previous allowance for `username` conflicted with
[design rules, section A](../../../00-design-rules.md#a-sensitive-data--log-redaction)
and the PII handling rule in [log safety](../../../03-coding/01-log-safety.md).
D0036 corrects the shared guidance and the BasicAuth and HmacAuth warning fields.
The broader credential-copy and disclosure audit is recorded in
[ownership evidence](../../security/review-evidence/credential-copy-ownership.md);
these scoped corrections do not establish that every credential surface conforms.

When reviewing the `resolve_credential/*.rs` handlers, **reject**:

- **fail-fast** (returning an error / rejecting the EdgionPlugins resource): one
  bad partner Secret would take down the whole plugin, violating the "management
  convenience" framing.
- **last-wins**: ordering is HashMap-iteration-dependent and unpredictable across
  runs, which makes incident analysis harder.
- **Silent drop**: the warn line is the only audit trail; do not remove it.

Implementation (KeyAuth):
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/key_auth.rs`

## Secret data safety

The raw API key is sensitive. `tracing::*` in `resolve_credential/key_auth.rs`
(first-wins warn or any future branch) must NOT emit the key value — allowed:
`secret_key`, `group`, consumer header name; forbidden: the raw key bytes (literal
or hashed). The per-plugin secret-safety contracts for the other credential types:
[basic-auth (`password`)](../../basic-auth/mandatory-design/dup-key-first-wins.md),
[hmac-auth (`secret` bytes)](../../hmac-auth/mandatory-design/dup-key-first-wins.md).

## Related

- `tasks/todo/plugin-grouping/01-key-auth-spec.md` §3 — decision record
- Sibling mirrors: [basic-auth](../../basic-auth/mandatory-design/dup-key-first-wins.md),
  [hmac-auth](../../hmac-auth/mandatory-design/dup-key-first-wins.md).
