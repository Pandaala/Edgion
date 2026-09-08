---
name: dup-key-first-wins
description: Use when reviewing BasicAuth Secret resolution, first-wins duplicate handling, and credential-free warning fields.
knowledge_type: mandatory-design
---

# BasicAuth — duplicate usernames are first-wins with credential-free warnings

## Conclusion and scope

The first-wins + warn policy (reject fail-fast / last-wins / silent-drop) is the
shared rule for the grouping series — see the canonical statement in
[key-auth/dup-key-first-wins.md](../../key-auth/mandatory-design/dup-key-first-wins.md). For BasicAuth
the duplicated identity is the **username**, stored as the key of
`BasicAuthConfig.resolved_users`. Cross-group warnings carry `secret_key`
(the rejected Secret pointer), `existing_group`, and `incoming_group`.
Within-group warnings carry `secret_key` and `group`. Both retain the enclosing
EdgionPlugins resource key, but neither emits the username or password.

Implementation:
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/basic_auth.rs`

## Rationale and evidence

The authority is [design rules, section A](../../../00-design-rules.md#a-sensitive-data--log-redaction)
and [log safety, iron rule 1](../../../03-coding/01-log-safety.md).
Secret usernames can contain PII; formatting the map key directly into tracing
extends its exposure to log storage. Redacted Debug on the map value cannot
protect an explicitly formatted username key.

`ResolvedBasicAuthUser.password: String` holds either plaintext or htpasswd-format
hash (`{SHA}<base64>` / `$apr1$...`). Any `tracing::*` in resolve_credential or the
gateway plugin must NOT carry the password value:

- Allowed in Controller duplicate warnings: resource/Secret pointers and group names
- Forbidden: raw username, `password = %password`, `password_hash = %...`, or any
  field that emits the password (literal or even hashed — the hash is still sensitive)

## Alternatives and limits

D0036 found that both duplicate-warning branches explicitly emitted
`username = %username`. The earlier version of this page allowed that field;
that allowance conflicted with the canonical redaction rules and is withdrawn.
Removing only those fields preserves both warnings and first-wins selection;
the operator still has the rejected Secret pointer and group context.

This correction does not alter `ResolvedBasicAuthUser.password`, transport,
Gateway verification, or credential ownership. It does not erase historical logs
or prove that every credential copy has a bounded lifetime. Operator-authored
resource and group names remain diagnostic metadata; do not embed credentials in
those names. Gateway authentication-success identity logging follows the separate
access-log masking contract.

## Verification and re-evaluation

`basic_auth_duplicate_warnings_omit_credentials_and_preserve_first_wins` exercises
the actual EdgionPlugins parse path with within-group and cross-group duplicates.
It checks both warning branches, Secret/group attribution, absence of username
and password values, first-wins contents, and reference registration. Execution
results are maintained in the active task; existing selection tests are retained.

Re-evaluate when resolver logging, identity fields, first-wins handling, or log
formatting changes. The complete credential-copy audit and HmacAuth log exposure
remain open in [D0036's task](../../../../tasks/todo/audit-auth-credential-copy-lifetimes.md).

## Resolution references

- D0036 — scoped BasicAuth duplicate-warning correction; the parent audit remains open.

## Related

- Canonical first-wins rule + sibling mirrors:
  [key-auth/dup-key-first-wins.md](../../key-auth/mandatory-design/dup-key-first-wins.md)
- `tasks/todo/plugin-grouping/05-basic-auth-spec.md` §3, §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
