---
name: review-tls
description: Review knowledge for tls, grouped by decision type.
---

# tls review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [tls-F010 (cert/key install dup) — Shared `install_cert_key_to_ssl` extraction deferred](intentional-design/cert-key-install-helper-deferred.md)
- [tls-F007 (duplication) — Two SNI matchers stay separate; semantics changes apply to both](intentional-design/cert-matcher-duplication-accepted.md)
- [tls-07 — `revocationCheck` / `ocspStapling` are reserved schema, not implemented](intentional-design/revocation-config-not-implemented.md)
- [Rustls Gateway Backend — FUTURE PRODUCT SCOPE](intentional-design/rustls-gateway-backend-unsupported.md)
- [Rustls Upstream Custom CA and mTLS — UNSUPPORTED](intentional-design/rustls-upstream-ca-mtls-unsupported.md)
- [Rustls Upstream Additional SAN Enforcement — UNSUPPORTED](intentional-design/rustls-upstream-san-unsupported.md)
- [`EdgionTls.min_tls_version` Allowing `TLS1_0` / `TLS1_1`: Deliberate Opt-In, Not Rejected](intentional-design/tls-version-operator-choice.md)

## Compatibility constraints

- [tls-02 — TLS 1.0/1.1 remain admissible via `min_tls_version`](compatibility-constraints/deprecated-tls-version-admission.md)

## Regression guards

- [TLS log backend-address masking and plaintext SNI scope](regression-guards/tls-log-address-redaction.md)

- [TLSRoute stage isolation and explicit empty configuration](regression-guards/tls-route-stage-isolation.md)
- [Gateway backend client identity does not select TLS](regression-guards/backend-client-identity-tls-selection.md)
- [Backend TLS connection-pool trust and identity isolation](regression-guards/backend-tls-pool-isolation.md)
- [EdgionTls incremental and atomic publication](regression-guards/edgion-tls-incremental-atomic-publication.md)
- [tls-08 — Backend (upstream) TLS CA bundle loads every PEM block + fail-closed](regression-guards/backend-ca-bundle-multi-pem.md)
- [Boring 5 FIPS opt-in build and verification](regression-guards/boring-v5-fips-build-verification.md)
- [tls-06 — mTLS CA bundle loads every PEM block](regression-guards/ca-bundle-multi-pem.md)
- [tls-F002 — cipher config failure is now fail-closed (no silent default-suite fallback)](regression-guards/cipher-fail-closed.md)
- [tls-F007 — TLS/SNI matchers select deterministically on same-hostname collisions](regression-guards/cross-gateway-sni-collision-deterministic.md)
- [DynamicExternalUpstream rejects unsafe explicit SNI and Host overrides — FIXED (D0045)](regression-guards/dynamic-external-upstream-sni-host-validation.md)
- [tls-F009 — `check_san_matches` vacuously passes for empty declared hosts](regression-guards/empty-hosts-san-vacuous-pass.md)
- [fr-cm2-01 — EdgionTls cross-namespace Secret refs are ReferenceGrant-gated](regression-guards/fr-cm2-01-crossns-secret-refgrant.md)
- [fr-grt-01 — admin API EdgionTls serialization scrubs private-key material](regression-guards/fr-grt-01-admin-api-secret-scrub.md)
- [tls-FB01 — Gateway `frontendValidation` silently installed empty CA store on missing `ca.crt`](regression-guards/frontend-mtls-empty-store-on-missing-ca-key.md)
- [tls-F010 — Gateway `frontendValidation` mislabeled `is_mtls = true` with empty CA refs](regression-guards/frontend-validation-is-mtls-empty-ca.md)
- [tls-F004 — `handshake_complete_callback` fallback intentionally skips `log_ssl`](regression-guards/handshake-complete-fallback-no-double-log.md)
- [tls-F011 — mTLS verify-setup is now fail-closed (no silent "anonymous accepted" downgrade)](regression-guards/mtls-config-reorder.md)
- [mtls-leaf-only-whitelist-verification — leaf-only SAN/CN allowlist](regression-guards/mtls-leaf-only-whitelist-verification.md)
- [tls-04 — Gateway listener serves all certificateRefs (RSA + ECDSA)](regression-guards/multi-certificate-ref.md)
- [passthrough-F001 — TLS passthrough PP2 dst built from bare IP](regression-guards/pp2-dst-bare-ip.md)
- [tls-F003 — Runtime expiry recheck is required on both cert-install paths](regression-guards/runtime-expiry-recheck-symmetry.md)
- [SNI Passthrough: ClientHello read contract](regression-guards/sni-passthrough-clienthello-read.md)
- [tls-F001 — `configure_mtls` returns before CA loading for `Terminate` mode](regression-guards/terminate-mtls-ca-ordering.md)
- [Typed SAN display prefixes are not DNS](regression-guards/typed-san-display-prefix-not-dns.md)
- [tls-05 — Upstream TLS ALPN for gRPC backends is set by the gRPC peer path, not `create_tls_peer` — FALSE-POSITIVE (primary) + ACCEPTED-TRADEOFF (residual)](regression-guards/upstream-grpc-tls-alpn-set-by-grpc-peer-path.md)
- [Wildcard Depth: Three Layers, Two Standards](regression-guards/wildcard-depth-gateway-api-vs-rfc6125.md)

## Review evidence

- [tonic ClientTlsConfig Not Calling domain_name() is Not a Vulnerability](review-evidence/tonic-domain-name-false-positive.md)
