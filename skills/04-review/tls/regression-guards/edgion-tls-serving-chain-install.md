---
name: edgion-tls-serving-chain-install
description: EdgionTls compiles its complete ordered serving chain once per accepted generation and installs it atomically, with fail-closed SSL identity replacement.
knowledge_type: regression-guards
---

# EdgionTls complete serving-chain installation

**Status:** fixed · 2026-09-21 · cold-compiled since D0014

## Invariant

EdgionTls must serve the complete ordered chain from its `tls.crt` value. A
root-only client must receive the intermediate certificates needed to validate
the leaf.

The chain, the private key, the optional mTLS client CA store, and the operator
cipher string are compiled when `TlsStore` publishes an accepted generation
(`TlsStore::validated_entry` -> `CompiledEdgionTls::compile`). The handshake path
performs no Secret byte copy, no PEM parsing, no ASN.1 decoding, and no
trust-store construction.

Scope of "once per generation": a partial update compiles only the keys it
touches and shares every untouched key's `Arc<CompiledEdgionTls>` unchanged. A
**relist** (`replace_all_tls`, reached via `ConfHandler::full_set` when
`CacheData::reset` runs) recompiles every matchable entry regardless of whether
its content changed. That cost is accepted deliberately, and memoization is not
a free win — see
[relist-recompiles-every-generation.md](../intentional-design/relist-recompiles-every-generation.md)
before proposing one.

`TlsCallback::apply_edgion_tls_cert` preserves this order:

1. Apply FIPS curve policy, then the compiled operator cipher (or the FIPS
   default set). A failure returns before anything is bound.
2. Apply the mTLS verification policy from the compiled client CA store. A
   failure returns before anything is bound.
3. Clear any serving identity inherited by the per-connection SSL.
4. Consume the compiled identity. A compilation failure stamps its static
   category and returns — after step 3, never before it.
5. Recheck the compiled leaf's Unix validity bounds.
6. Install the compiled chain and key atomically through
   `install_compiled_serving_identity` (`SSL_certs_clear` +
   `SSL_set_chain_and_key`), which clears again on failure.

Read, parse, validity, and installation failures use fixed non-secret
diagnostic categories. The certificate, key, validity dates, and BoringSSL error
stack must not be copied into logs.

## Why the entry clear must precede the compiled-identity check

The certificate callback returns `()`, so it cannot directly return a fatal TLS
error. Leaving no usable server identity is its fail-close mechanism. Entry
clearing prevents material that failed to compile from falling back to an
identity inherited from the SSL context. Because a compilation failure returns
early (step 4), the clear in step 3 must stay ahead of it; relying only on the
clear inside the installer would leave an inherited identity bound on that path.
The installer's own clear-then-set contract covers a failed replacement.

## Compilation failure does not remove the resource from the matcher

`validate_cert` accepts a resource whose mTLS CA reference is present but whose
controller-resolved `ca.crt` content is missing or unparseable
(`validate_mtls_config` checks only `ca_secret_ref.is_some()`). Such a resource
compiles with an error and **keeps its hostname slot and collision precedence**,
failing the handshake closed. Dropping it from the index would silently promote
the next candidate for that hostname — a security-relevant behavior change. This
is why the compiled fields are `Result`, not `Option`.

The serving-identity compile errors are defence-in-depth rather than a reachable
production path: `validate_cert` runs `check_key_matches_cert`, which itself
calls `parse_strict_certificate_chain` and `parse_strict_private_key_pem`, so a
matchable entry always has a strict-parseable chain and key.

## Diagnostic granularity

Atomic installation reports one category (`set serving identity`) instead of the
former per-step `set certificate failed` / `set certificate chain failed` /
`set private key failed`. This matches the Gateway Listener path, which already
collapsed the same failures. The distinction was only observable for a
key/certificate mismatch, which `validate_cert` rejects at publication, so the
granularity loss applies to a state unreachable in production. A CRYPTO_BUFFER
allocation failure folds into `parse certificate failed` for the same symmetry
reason.

## Verification

Regression coverage uses real loopback TLS sessions. One case fails strict chain
parsing after preinstalling a valid prior identity, proving the entry clear
still runs ahead of the early return. A second proves the new leaf/intermediate
fixture succeeds with its correct key, then verifies that a mismatched key
produces the static install diagnostic and leaves `SslRef::certificate()` empty.
A third proves an unrenderable operator cipher aborts before any identity is
bound. Store-level coverage proves an uncompilable CA keeps its index position
and that an untouched key keeps its compiled generation allocation across a
partial update.

## Ownership boundary

Both downstream serving paths are now cold-compiled and share
`install_compiled_serving_identity`. They remain separate functions because
their surrounding policy differs: EdgionTls owns the resource's cipher and
`clientAuth` branches, while the Gateway Listener path owns dual-certificate
selection by client signature algorithm and `frontendValidation`. See
[superseded/cert-key-install-helper-deferred.md](../superseded/cert-key-install-helper-deferred.md)
for the retired deferral of that shared installer.

## Re-evaluation triggers

- The Pingora callback gains a direct fatal-error return.
- BoringSSL identity replacement or chain ownership APIs change.
- A new downstream serving-certificate source is added.
- `validate_cert` starts validating resolved CA content, which would change the
  reachability argument above.
