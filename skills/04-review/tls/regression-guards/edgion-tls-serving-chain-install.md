---
name: edgion-tls-serving-chain-install
description: EdgionTls strictly parses and serves every certificate in its ordered leaf-plus-intermediate chain, with fail-closed SSL identity replacement.
knowledge_type: regression-guards
---

# EdgionTls complete serving-chain installation

**Status:** fixed · 2026-09-21

## Invariant

`TlsCallback::apply_edgion_tls_cert` must serve the complete ordered chain from
the EdgionTls `tls.crt` value. A root-only client must receive the intermediate
certificates needed to validate the leaf.

The callback preserves this order:

1. Apply FIPS cipher/curve policy and mTLS verification policy.
2. Clear any serving identity inherited by the per-connection SSL.
3. Strictly parse the complete certificate chain and private key before binding.
4. Recheck the leaf's strict-parser Unix validity bounds.
5. Install the leaf, append intermediates in source order, and install the key
   last.
6. Clear the SSL identity again if any installation step fails.

Read, parse, validity, and installation failures use fixed non-secret
diagnostic categories. The certificate, key, validity dates, and BoringSSL error
stack must not be copied into logs.

## Why both clears are required

The certificate callback returns `()`, so it cannot directly return a fatal TLS
error. Leaving no usable server identity is its fail-close mechanism. Entry
clearing prevents malformed selected material from falling back to an inherited
identity. Failure clearing prevents a partially installed replacement from
remaining available after a later chain or key operation fails.

Regression coverage uses real loopback TLS sessions. One case fails strict
chain parsing after preinstalling a valid prior identity. A second proves the
new leaf/intermediate fixture succeeds with its correct key, then verifies that
a mismatched key produces the static key-install diagnostic and leaves
`SslRef::certificate()` empty.

## Ownership boundary

This guard applies to the EdgionTls runtime PEM path. Gateway Listener TLS keeps
its separate cold-compiled `CompiledServingCertificate` and atomic
`SSL_set_chain_and_key` installation. Moving EdgionTls compilation to its
publication generation is deferred as D0014 and is outside this fix.

## Re-evaluation triggers

- The EdgionTls identity is compiled before the handshake path.
- The Pingora callback gains a direct fatal-error return.
- BoringSSL identity replacement or chain ownership APIs change.
- A new downstream serving-certificate source is added.
