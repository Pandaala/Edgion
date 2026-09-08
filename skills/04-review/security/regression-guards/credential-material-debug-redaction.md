---
name: credential-material-debug-redaction
description: Review credential material Debug independently from admin serialization, enclosing state redaction, and zeroization.
knowledge_type: regression-guards
---

# Credential material Debug redaction

## Conclusion and scope

The material types in `edgion-resources/src/credentials/materials.rs` redact their
own Debug output, including alternate formatting. `ResolvedBasicAuthUser` also
redacts its password/hash and group. Debug formatting must not delegate to raw
credential fields, even when an enclosing `CredentialState` already redacts them.

This correction covers direct Debug and nested formatting of these types. It does
not make arbitrary resource Debug safe: map keys, sibling credential types, raw
Secrets, explicitly formatted fields, and serialization need independent review.

## Rationale and evidence

D0036's credential-copy audit found derived Debug on `BasicAuthMaterial`,
`EsApiKeyMaterial`, `HmacMaterial`, `WebhookApiKeyMaterial`, `AiApiKeyMaterial`,
`AcmeAccountMaterial`, `TlsMaterial`, and `ResolvedBasicAuthUser`. Passwords,
private keys, and identity-bearing certificate content could be formatted
directly. This is a formatting exposure, not evidence of an observed production
log disclosure. `BearerMaterial` already redacted its token.

The authority is [log safety](../../../03-coding/01-log-safety.md) and
[design rules](../../../00-design-rules.md), section A. The fix replaces derived
Debug with fixed redacted output without changing field types or credential
consumers. `ResolvedBasicAuthUser` retains real serde transport values, while
`BasicAuthConfig.resolved_users` keeps its existing admin redaction adapter and
schema exclusion.

## Alternatives and limits

`Zeroizing` is a drop-time buffer mechanism, not a Debug redactor. No new
zeroization guarantee is introduced. Source Secrets, configuration generations,
serde/protobuf/HTTP buffers, allocator residue, and explicitly cloned fields are
outside this fix. Generation owners and detached blocking authentication work
must not be invalidated merely to shorten credential retention.

## Verification and re-evaluation

Targeted regression tests:

- `credentials::materials::tests::credential_material_debug_never_formats_fields`
  covers application credentials and ACME/TLS material, including byte buffers
  and alternate Debug output.
- `resolved_password_debug_redaction_preserves_transport` covers plaintext and
  hashed BasicAuth passwords, nested Debug, real transport round trips, and
  mandatory admin serialization redaction.

Execution results are recorded in the active task. Re-evaluate when material
types, Debug implementations, enclosing containers, or credential serialization
change. Direct field formatting remains subject to log-safety review.

## Resolution references

- D0036 — partial correction; the complete copy/lifetime audit remains open in
  [the canonical task](../../../../tasks/todo/audit-auth-credential-copy-lifetimes.md).
