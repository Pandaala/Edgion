---
name: jwe-resolved-credential-failure-policy
description: Use when reviewing JWE Secret resolution, resolvedCredential validation, runtime construction, or invalid-key publication behavior.
knowledge_type: regression-guards
---

# JWE resolved-credential failures remain authentication failures

## Conclusion and scope

Each `JweDecryptConfig` names exactly one operator-facing `secretRef`. The
Controller-owned `resolved_credential` is transport state and cannot substitute
for that reference during `PluginValidation`.

A Secret that is missing `data.secret`, has invalid inner Base64 when
`base64Secret` is enabled, or does not produce the 32-byte key required by
A256GCM is a resolved-reference failure, not an operator configuration-shape
failure. The Controller reports it through `ResolvedRefs=False`; the Gateway
still constructs a `JweDecrypt` with an unavailable `PreparedDecryptor` state.
Requests fail closed with the plugin's generic 401 authentication response.

## Rationale and evidence

The project authentication-failure rule requires an unusable authentication
Secret to use the plugin's generic authentication denial rather than a 500 that
reveals server misconfiguration. `JweDecryptConfig::resolved_key_material`
provides one bounded credential classifier for Controller status and Gateway
construction. Operator validation considers only `secretRef` and the other
operator-facing fields.

`resolve_credential::jwe::resolve` clears the previous generation before every
resolution and records a present Secret even when its required key is absent.
The status projection can therefore distinguish present-but-unusable material
from missing or unauthorized references, which remain covered by the shared
dependency classifier.

## Alternatives and limits

JWE does not have a credential group or multiple `kid` candidates. There is no
partial-good key subset: the generation contains one prepared cipher or one
unavailable state. Skipping the plugin would bypass authentication.

An invalid Secret update replaces the previous runtime rather than retaining a
last-known-good key. Retention could continue accepting a credential the
operator intended to revoke. Requests already holding an older immutable
generation may finish with it; new requests use the replacement publication.
The whole-resource DSL/WAF last-known-good path is disabled whenever either the
incoming or candidate generation contains an enabled JWE entry. Retaining the
whole entry would also retain its key and outer execution conditions, so
configuration equality is deliberately not used as an authorization boundary.

Malformed compact-JWE structure and unsupported protected-header algorithms
remain request errors (400). Missing credentials, invalid key material, a
missing required token, and ciphertext authentication failure use generic 401
responses. No response or PluginLog entry contains credential bytes, decoder
details, Secret names, or key identifiers.

## Verification and re-evaluation

Focused tests cover operator validation isolation, resolved-key classification,
Controller status mapping, Gateway construction, and generic authentication
responses. Re-evaluate if JWE gains multiple selectable keys, runtime key
rotation independent of plugin publication, or a different content-encryption
algorithm with another key length.

## Resolution references

- `jwe-resolved-credential-auth-failure` — corrected the pre-construction HTTP
  500 and established single-key fail-closed publication.
