# TLS Configuration

How to terminate TLS at the Gateway, add advanced TLS features, and automate certificate issuance.

- [TLS Termination](./tls-termination.md) — standard Gateway API listener termination (`tls.mode: Terminate`)
- [EdgionTls](./edgion-tls.md) 🔌 — extension for mTLS, minimum TLS version, and cipher control
- [ACME Automatic Certificates](./acme.md) 🔌 — automatic issuance and renewal via the ACME protocol
- [FIPS Mode](./fips.md) 🔒 — FIPS-validated BoringSSL module and approved-by-default cipher/curve set

For forwarding encrypted TLS without decrypting it, see [TCP / TLS Listener](../listeners/tcp.md) (passthrough mode) and [TLSRoute Overview](../../../user-guide/tls-route/overview.md).
