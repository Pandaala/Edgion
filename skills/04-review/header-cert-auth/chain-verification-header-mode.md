---
name: chain-verification-header-mode
description: Decision rule for HeaderCertAuth Header-mode certificate chain verification findings. Read this file first when reviewing "Header mode only does single-layer CA matching, does not build full certificate chain" findings.
---

# HeaderCertAuth Header Mode — Single-Layer CA Matching NOT-AN-ISSUE

## Conclusion

The `verify_against_ca()` of `header_cert_auth` Header mode performs only single-layer CA matching (leaf-cert signature vs configured CA), without building a full X.509 path; classified as **NOT-AN-ISSUE**. Fix suggestions of the form "switch to webpki / implement chain-path construction" are not accepted.

## Core Rationale

**1. Architectural premise of Header mode**

Header mode's prerequisite is a TLS-terminating proxy in front (Nginx / Envoy / Traefik / HAProxy):
- The proxy has already performed full X.509 chain verification (root → intermediate → leaf) at the TLS handshake layer
- After verification passes, the cert is placed into a header (such as `ssl-client-cert` / `x-forwarded-client-cert`)
- The cert that Edgion reads from the header is already a cert trusted via the proxy

In this architecture, `verify_against_ca()` is **secondary filtering** ("which CA organization that I configured does this cert belong to"), not first-time security verification. Single-layer matching satisfies this purpose.

**2. Industry consistency**

Kong's `mtls-auth` plugin in header mode likewise only does single-layer CA matching, without redoing chain verification at the application layer. The industry model is: chain verification = TLS layer's responsibility; reading header at the application layer = trust that the proxy has verified.

**3. The real attack surface is at the deployment layer**

The actual security risk is whether the proxy strips the cert header from external requests (preventing the attacker from injecting a forged cert directly). This is a deployment-config issue, not an Edgion code issue:
- Nginx: `proxy_set_header ssl-client-cert ""` clears incoming headers before setting
- Envoy: automatically overrides the XFCC header; does not pass through external
- Documentation should explain that the operator must confirm proxy-stripping behavior

## Fix Suggestions Not Accepted

- Introduce `webpki` for path construction (rustls ecosystem; incompatible with the BoringSSL project, and redundant)
- Implement chain traversal manually with `x509_parser` (already verified by the proxy; redundant here; many implementation details, risk of new bugs)
- Require all configured CAs to be cross-validated (breaks the current flat CA bundle usage)

## Note: Connection Mode is Unaffected

The `mode=Connection` path performs full chain verification at the Pingora/BoringSSL TLS handshake layer, without going through `verify_against_ca()`; naturally correct.
