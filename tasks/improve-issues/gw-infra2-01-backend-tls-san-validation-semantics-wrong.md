# gw-infra2-01: BackendTLSPolicy `subjectAltNames` semantics are inverted/additive: hostname is still authenticated, and wildcard SANs in the backend certificate never match

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**: `edgion-gateway/src/tls/validation/backend_san.rs:32-75` (`validate_backend_sans`, line 48 `matches(hostname_pattern, cert_san)`), `edgion-gateway/src/backends/prepared_backend_tls.rs:36-49` (`san_hook`), `edgion-gateway/src/backends/peer_builder.rs:74-80,140-186` (`extract_tls_config`, peer construction), `edgion-gateway/src/routes/http/proxy_http/pg_connected_to_upstream.rs:20-35` (post-handshake SAN rejection), `edgion-gateway/src/tls/validation/outbound_san_verifier.rs:259-291` (rustls twin used by plugin outbound clients), Pingora fork `pingora-core/src/connectors/tls/boringssl_openssl/mod.rs:200-225` + `upstreams/peer.rs:582-583` (`verify_hostname: true` default)

## Summary
Gateway API (vendored v1.6.2 CRD, `BackendTLSPolicy.spec.validation.hostname`) says: hostname MUST be
used for SNI, and for authentication **unless `subjectAltNames` is specified; if SANs are specified,
hostname MUST NOT be used for authentication**. Edgion always keeps Pingora's hostname verification on
(the SNI = `validation.hostname` is added to the verify params) and then runs the SAN list as an
*additional* post-handshake filter. Independently, the SAN filter calls the RFC 6125 matcher with the
arguments reversed (policy value as the pattern, certificate SAN as the name), so a backend certificate
carrying a wildcard DNS SAN (`*.svc.example.com`) never satisfies an exact policy SAN
(`api.svc.example.com`). The two defects together reject exactly the deployments SAN validation exists
for (SPIFFE/URI-only workload certs, shared wildcard certs, SNI-routed backends whose cert does not list
the SNI name).

## Current behavior (evidence)
1. `peer_builder.rs:74-76` takes `sni = policy.spec.validation.hostname`; `peer_builder.rs:141-160`
   builds `HttpPeer::new(addr, true, sni)` / `new_mtls(...)` and never touches
   `peer.options.verify_hostname` or `alternative_cn` (grep: no `verify_hostname` in
   `edgion-gateway/src`). Pingora's default is `verify_cert: true, verify_hostname: true`
   (`upstreams/peer.rs:582-583`), and the BoringSSL connector then does
   `add_host(verify_param, peer.sni())` (`boringssl_openssl/mod.rs:207-210`). So the chain must
   validate **for the hostname** even when `subjectAltNames` is set.
2. `prepared_backend_tls.rs:36-49` installs the SAN list as a `HandshakeCompleteHook`;
   `pg_connected_to_upstream.rs:26-33` rejects after the handshake when it failed. This is additive
   (AND) to (1), not a replacement.
3. `backend_san.rs:48`: `matches(hostname_pattern /* policy */, cert_san /* certificate */)`.
   `rfc6125_hostname::matches(pattern, hostname)` (`edgion-algo/src/matcher/rfc6125_hostname/mod.rs:9-25`)
   only expands a wildcard in its *first* argument. A cert SAN `*.svc.example.com` versus a policy SAN
   `api.svc.example.com` → not equal, the policy value has no `*.` prefix → `false`. The unit tests
   (`edgion-gateway/tests/unit/tls/validation/backend_san_test.rs`) cover only policy-side wildcards;
   there is no case for a wildcard certificate SAN.
4. The same `validate_backend_sans` is reused by `OutboundSanVerifier` (`outbound_san_verifier.rs:282`),
   which first delegates to webpki with `server_name` (hostname verification again) — same AND semantics
   for LDAP / outbound plugin TLS.

Consequences:
- A mesh/SPIFFE backend whose cert has only `URI: spiffe://...` SAN, configured per spec as
  `hostname: backend.ns.svc` + `subjectAltNames: [{type: URI, uri: spiffe://...}]`, fails Pingora's
  hostname check before the SAN hook ever runs.
- A backend serving a wildcard cert fails whenever the operator lists the concrete name in
  `subjectAltNames` (the natural way to write the policy).
- Conformance (`BackendTLSPolicySANValidation` is listed in
  `edgion-tests/conformance/supported_features.txt`) evidently does not exercise "SAN matches, hostname
  does not" — the claim is broader than the implementation.

## Industry comparison
- Envoy (and Envoy Gateway's BackendTLSPolicy translation): `UpstreamTlsContext.sni` carries the
  hostname, while authentication uses `match_typed_subject_alt_names` **instead of** the SNI when SANs
  are configured; for DNS SANs an `exact` matcher is compared with RFC 6125 wildcard semantics against
  the certificate (`*.example.com` in the cert matches `exact: foo.example.com`).
- NGINX `proxy_ssl_name` (SNI) vs `proxy_ssl_verify` with `proxy_ssl_name` as the reference identity —
  one reference identity, cert-side wildcards honored by OpenSSL `X509_check_host`.
- Istio DestinationRule `subjectAltNames` replaces SNI-based identity checks for the same reason.

## Impact
Operators cannot use BackendTLSPolicy SAN validation for the main real-world cases (SPIFFE identities,
wildcard certs, SNI distinct from the cert identity). Failures surface as 502
`BackendTlsSanValidationFailed` or a generic handshake verify error, which looks like a cert problem on
the backend rather than a gateway semantics bug.

## Suggested direction
1. When `subjectAltNames` is non-empty, set `peer.options.verify_hostname = false` (keep
   `verify_cert = true` for chain/expiry) and let the SAN list be the sole identity check; keep the
   hostname check when SANs are empty. Mirror this in `OutboundSanVerifier` (verify chain via webpki
   without a name, or use a verifier that skips the name check only when policy SANs exist).
2. Fix the matcher direction for DNS SANs: an exact policy hostname must match a certificate SAN with
   RFC 6125 left-most-label wildcard rules (`matches(cert_san, policy_hostname)`), and keep the existing
   "policy wildcard vs cert name" only if Gateway API's `Hostname` type wildcard is intended to be
   accepted (it is, by CRD pattern) — i.e. accept either direction, never multi-label.
3. Prefer doing the SAN check inside the BoringSSL verify callback (fail during the handshake) instead
   of after `connected_to_upstream`, so a mismatching connection is never established or pooled and the
   error class is a TLS error.
4. Add unit cases: wildcard cert SAN vs exact policy SAN; URI-only cert with non-matching hostname.

## Notes
Checked `skills/04-review/tls/regression-guards/wildcard-depth-gateway-api-vs-rfc6125.md` (covers
wildcard depth, not argument direction or hostname-vs-SAN precedence),
`skills/04-review/tls/intentional-design/rustls-upstream-san-unsupported.md` (rustls Gateway backend; not
applicable), and `skills/04-review/backends/regression-guards/tls-policy-wildcard-cross-service.md`
(target isolation). No task under `tasks/` tracks this. Out-of-scope but adjacent: the upstream-authority
override (`pg_upstream_peer.rs:327-339`) rewrites `peer.sni`, which also changes the hostname used for
authentication away from `validation.hostname`; worth checking together.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged (Gateway API spec violation and loss of the main SAN use cases).
- `tls/validation/backend_san.rs:48` calls `matches(hostname_pattern, cert_san)`, and `edgion-algo/src/matcher/rfc6125_hostname/mod.rs:9-25` expands a wildcard only in the first (pattern) argument. A wildcard certificate SAN therefore never matches an exact policy SAN.
- `peer_builder.rs:74-80,140-167` builds `HttpPeer::new/new_mtls(addr, .., sni = validation.hostname)`. `verify_hostname` and `alternative_cn` are never set (no hit in `edgion-gateway/src`), and the fork default is `verify_hostname: true` (`pingora-core/src/upstreams/peer.rs:583`). `:184` installs the SAN list only as an additional handshake-complete hook.
- The vendored CRD `config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml:320-322` says hostname is used for authentication "unless SubjectAltNames is specified" and then "MUST NOT be used for authentication".
- `tls/intentional-design/rustls-upstream-san-unsupported.md` describes SANs as "additional" constraints, but only for the unsupported rustls backend. It records no decision to deviate from the spec on BoringSSL. That wording should be corrected when this is fixed.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `tls/validation/`, `backends/prepared_backend_tls.rs`, `backends/peer_builder.rs`, `pg_connected_to_upstream.rs` and `edgion-algo/src/matcher/rfc6125_hostname/` are unchanged between feature-08-30 and HEAD.
- `backend_san.rs:48` still calls `matches(hostname_pattern, cert_san)` (policy value as pattern), and `grep verify_hostname|alternative_cn` over `edgion-gateway/src` still returns nothing, so Pingora's default hostname verification remains in addition to the SAN hook.
- The only change to the related TLS review pages (`backend-tls-pool-isolation.md`) adds test-publication notes; no page accepts the AND semantics.
