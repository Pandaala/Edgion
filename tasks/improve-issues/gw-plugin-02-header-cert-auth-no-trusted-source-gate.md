# gw-plugin-02: HeaderCertAuth Header mode accepts a public certificate from any peer (no trusted-source gate)

- **Category**: design-error
- **Severity**: P2 (security hardening; downgraded from P1 in independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs:231-287` (`verify_leaf_der`, `extract_from_header`), `:329-404` (`run_request`); `edgion-resources/src/resources/edgion_plugins/plugin_configs/header_cert_auth.rs:70-128` (no source-trust field)

## Summary
In `mode: Header`, the plugin parses the certificate from the client-supplied header and checks
only that a configured CA signed it. A leaf certificate is public data: it travels in clear in
TLS 1.2 handshakes and appears in logs, CT-like inventories, and browser and devtools exports.
Possession of the certificate proves nothing without the private key. The only thing that makes
Header mode safe is that the header can come only from a trusted TLS terminator. The plugin has
no way to check that: it has no trusted-peer list and does not consult `DirectPeerIp`. Anyone who
can reach a listener with this plugin attached, and has ever seen a valid client cert, can
authenticate as that identity.

## Current behavior (evidence)
- `extract_from_header` (`:262`) reads `certificate_header_name` from the request, base64- or
  URL-decodes it, and calls `verify_leaf_der`.
- `verify_leaf_der` (`:231-260`) checks validity and the issuer-DN match plus the signature against
  the CA pool. It runs no peer check and has no proof-of-possession, and none is possible in this mode.
- `run_request` (`:329`) runs this path for every request regardless of
  `session.direct_peer_ip()` / transport. `HeaderCertAuthConfig` has no `trustedSources`,
  `ipRefs`, or similar field.
- The only mitigation is prose:
  `docs/en/user-guide/http-route/filters/edgion-plugins/header-cert-auth.md:173`
  ("ensure only the trusted TLS terminator can set certificateHeaderName; strip that header from
  untrusted ingress").

## Industry comparison
- **Kong `header-cert-auth`** ships `secure_source` (default **true**) with `trusted_ips`. Header
  certificates are accepted only from trusted peers, so the plugin enforces it rather than relying
  on documentation.
- **Envoy XFCC**: `forward_client_cert_details` defaults to `SANITIZE`, so a client-supplied
  `x-forwarded-client-cert` is removed unless the hop is explicitly trusted (`APPEND_FORWARD` /
  `SANITIZE_SET` on a trusted listener).
- **Traefik `passTLSClientCert`** builds the header from the real TLS connection; it never trusts
  an inbound one.

## Impact
Any deployment where the gateway listener is reachable by anything other than the TLS terminator
is open to complete authentication bypass with a replayed public certificate. Examples: a
TCP-passthrough LB, NodePort/hostNetwork exposure, in-cluster callers, or a second listener sharing
the route. The plugin is also commonly the *only* auth on such routes. Because the CA check
succeeds, the access log shows `cert verify:ok`, which hides the attack.

## Suggested direction
1. Add a required (Header-mode) source-trust field that reuses the canonical IP-list machinery:
   `trustedSourceRefs` (IpList references, matched on **`DirectPeerIp`**, never `RemoteIp`).
   Reject or treat as absent (anonymous path) any certificate header from an untrusted peer. When
   the header is present from an untrusted peer, also strip it before forwarding.
2. Make the field mandatory in Header mode, the same way `caSecretRefs` already is, so the secure
   configuration is the only valid one. Pre-release status means no compatibility shim is needed.
3. Optionally support an XFCC-style "trusted listener" alternative for deployments where the
   terminator's address is unstable.

## Notes
- `skills/04-review/header-cert-auth/review-evidence/chain-verification-header-mode.md` closes
  *chain-building* findings and states that "the real attack surface is at the deployment layer".
  This finding is a different one. It argues that the plugin must enforce that deployment
  invariant, as Kong and Envoy do by default, because CA verification of a header certificate
  gives no proof of possession. It does not ask for chain building.
- `certificate-identity-header-projection.md` covers output-header clearing, not input trust.
- Reuse rules: CLAUDE.md cross-cutting rules 6 and 12 (`radix_ip`, `DirectPeerIp` for security
  decisions) and plugin IP-list ownership (IpList references only).

## Verification (independent)

- **Verdict**: DOWNGRADED (P1 → P2)
- Claims hold: `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs` has no `direct_peer_ip`/trusted-source check, and `HeaderCertAuthConfig` (`edgion-resources/.../plugin_configs/header_cert_auth.rs:70-128`) has no source-trust field; the CA signature check is the only gate.
- `skills/04-review/header-cert-auth/review-evidence/chain-verification-header-mode.md:26-30` records header trust as a deployment-layer responsibility and characterizes Kong as doing only CA matching. Kong's `header-cert-auth` `secure_source` (default true) with `trusted_ips` is new evidence against that characterization, so the finding is not closed by the page.
- Why lower: Header mode is opt-in and documented (`docs/en/.../header-cert-auth.md:173`). A `DirectPeerIp` allow-list protects only bypass paths (NodePort, in-cluster callers, second listener); with the usual TLS-terminating LB in front, all traffic already comes from the trusted peer, so safety still rests on the terminator overwriting the header. It is valuable defense-in-depth with a secure default, not a P1 default-config bypass.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `header_cert_auth/plugin.rs` (`verify_leaf_der` at `:231`, `run_request` at `:329`) and `plugin_configs/header_cert_auth.rs` are unchanged; there is still no trusted-source gate.
- No header-cert-auth review page changed.
