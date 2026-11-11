# System CA Trust Unified on the OS CA Bundle (Breaking Change)

**Release Date**: 2026-11-10  
**Type**: Breaking Change  
**Affects**: Gateway startup, Gateway backend TLS and every Gateway outbound TLS client, Controller outbound TLS (etcd conf center, OAuth2 credential source, HTTPDNS resolver), `edgion-cli` HTTPS

## Summary

`wellKnownCACertificates: System`, or an omitted CA, now has one meaning everywhere: the
OS CA bundle at a fixed path, the first existing of:

1. `/etc/ssl/certs/ca-certificates.crt` (Debian, Ubuntu, Alpine, shipped images)
2. `/etc/pki/tls/certs/ca-bundle.crt` (RHEL family, including the Rocky packages)
3. `/etc/ssl/cert.pem` (macOS)

There is no setting for it, and `SSL_CERT_FILE` does not select it. Custom trust is a
Kubernetes CA reference (`caCertificateRefs`), which replaces `System`. Backend
verification also consults BoringSSL's default certificate locations; see
[Backend TLS](../user-guide/http-route/backends/backend-tls.md).

## Behavior changes

**Previously:**
- Backend TLS (`BackendTLSPolicy` `System`) used the OS CA store through BoringSSL.
- Gateway outbound clients (OIDC/JWKS, LDAP, ext_proc, forward-auth, webhooks, LinkSys
  Redis/Kafka/Elasticsearch/OTLP/HTTPDNS, encrypted health checks) used a Mozilla root
  list compiled into the binary.
- A private root added to the image was trusted by backends only.
- etcd with `System` trust, or an `https://` etcd without a `tls` block, had no roots and
  could not verify any server.

**Now:**
- Backend TLS and every outbound client load the same bundle file. A root added to the
  OS bundle (`update-ca-certificates`) is trusted by both.
- The Gateway refuses to start when no bundle exists, or it is not a regular file, exceeds
  8 MiB, has malformed PEM, contains no usable certificate, or cannot be loaded by BoringSSL.
- The Controller loads the bundle at startup and logs it; without one it logs a WARN and its
  `System`-trust connections fail.
- `edgion-cli` verifies `https://` servers against the bundle and warns when none exists.
- Outbound trust follows the OS distribution's root program instead of the compiled Mozilla
  list. Roots the distribution still ships are trusted; Mozilla's external name constraints
  are not applied.

## Migration

- Shipped images and packages already install `ca-certificates`; no action is needed.
- Custom images must install `ca-certificates` (or provide a PEM bundle at one of the paths).
- To trust a private CA, reference it with `caCertificateRefs` on the `BackendTLSPolicy`, on
  the resource, or in `EdgionGatewayConfig.spec.outboundTls.validation`.

## Related documentation

- [Backend TLS](../user-guide/http-route/backends/backend-tls.md)
- [Gateway configuration: System CA bundle](../ops-guide/configuration/gateway-config.md)
- [Connectors (LinkSys)](../ops-guide/infrastructure/connectors.md)
