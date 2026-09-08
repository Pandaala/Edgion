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

**Scoped correction (D0036):** `hmac.rs::resolve_group` omits raw `username`
from its short-secret warning and both duplicate-warning branches. The previous
username allowance conflicted with canonical PII redaction, not an accepted
tradeoff. Authority: [design rules, section A](../../../00-design-rules.md#a-sensitive-data--log-redaction)
and [log safety](../../../03-coding/01-log-safety.md).

The resolved map's username key still selects `HmacCredential.secret`, with
`group` preserving the first retained source and `headers` preserving permitted
upstream metadata. Only the log projection changes. The same resolver serves
standalone HmacAuth and `Condition::HmacAuth`; condition resolution ignores
credential metadata. Short-secret warnings retain `secret_len` / `min_len` and
occur only for retained entries. No authentication policy or lifetime changes.

Implementation:
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac.rs`

## Secret data safety iron rule (HmacAuth-specific)

`HmacCredential.secret: Vec<u8>` is the HMAC shared key. Any `tracing::*` in this
file (first-wins, length warning, parse failure, future branches) must NOT carry
the secret value:

- Allowed: `secret_key`, `group`, `secret_len` (numeric), `min_len`
- Forbidden: raw username, `secret = %secret_value`, `secret_bytes = ?secret_value.as_bytes()`,
  any field that emits secret content (literal or hashed)

Review all direct fields and error formatting, not only fields named `secret`.
The focused regression does not prove parse-error formatting, derived Debug,
map keys elsewhere, or the entire credential lifecycle safe. Those remain within
[the credential-copy audit task](../../../../tasks/todo/audit-auth-credential-copy-lifetimes.md).

## Verification and re-evaluation

`hmac_warnings_omit_credentials_and_preserve_first_wins` captures actual Controller
parse logs for standalone HmacAuth and a shared HMAC condition. It covers a retained
short secret, within-group and cross-group duplicates, omitted identities/keys,
resource attribution, lengths, first-wins contents, metadata policy, and Secret
reference registration. Execution results are recorded in the active task.
Re-evaluate when resolver branches, credential field projection, or shared HMAC
consumers change. Historical log retention and allocator erasure are outside scope.

## Envoy comparison and limits

Envoy BasicAuth's [configuration parser](https://github.com/envoyproxy/envoy/blob/main/source/extensions/filters/http/basic_auth/config.cc)
rejects duplicate users with a fixed diagnostic that omits the identity. Its
[request filter](https://github.com/envoyproxy/envoy/blob/main/source/extensions/filters/http/basic_auth/basic_auth_filter.cc)
borrows username/password views from one decoded buffer and uses generic credential
failure text. This is a BasicAuth comparison, not evidence of an equivalent grouped
HMAC resolver or an end-to-end zeroization guarantee. Edgion retains its canonical
first-wins plus warning policy; Envoy's rejection policy is not adopted.

## Related

- Canonical first-wins rule + sibling mirrors:
  [key-auth/dup-key-first-wins.md](../../key-auth/mandatory-design/dup-key-first-wins.md)
- `tasks/todo/plugin-grouping/02-hmac-auth-spec.md` §3 + §5.5 — decision record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L8 — pilot rationale
