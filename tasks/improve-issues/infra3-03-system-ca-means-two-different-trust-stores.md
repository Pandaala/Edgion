# infra3-03: `wellKnownCACertificates: System` resolves to two different trust stores in the same Gateway

- **Category**: design-error
- **Severity**: P3
- **Scope**: local
- **Location**: `edgion-gateway/src/backends/peer_builder.rs` (`create_tls_peer`, `use_system_ca` branch),
  Pingora fork `pingora-core/src/connectors/tls/boringssl_openssl/mod.rs:115,139` (`set_default_verify_paths`),
  `edgion-common/src/link_sys_core/tls.rs:104-106`, `edgion-gateway/src/tls/validation/outbound_client.rs:29-31`,
  `edgion-gateway/src/link_sys/providers/otlp/transport.rs:62`, `edgion-gateway/src/plugins/http/ext_proc/client.rs:462`,
  `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs:219`, `Cargo.toml:136` (`reqwest` 0.13.5, `rustls-no-provider`; roots supplied explicitly from the shared WebPKI root store)

## Summary
The same API value, `wellKnownCACertificates: System` (or an omitted CA), means different trust anchors
depending on which egress path uses it. For main-proxy backends (BackendTLSPolicy, Gateway backend TLS),
"System" means the operating-system trust store of the Gateway image. BoringSSL loads it through
`set_default_verify_paths`. For every Gateway-originated client (OIDC, LDAP, ext_proc, Webhook, OTLP,
Elasticsearch, Redis, Kafka, GeoIP updater, Wasm fetch), "System" means the `webpki-roots` Mozilla bundle
compiled into the binary. The two sets differ, so the same CA reference behaves differently from one
resource to the next.

## Current behavior (evidence)
- Backend path: `create_tls_peer` leaves `peer.options.ca = None` when `use_system_ca` is set. Only the
  `!use_system_ca` branch installs a custom chain. The Pingora connector then calls
  `builder.set_default_verify_paths()` (`boringssl_openssl/mod.rs:115,139`), which reads
  `/etc/ssl/certs`-style OS paths. The runtime images install `ca-certificates`
  (`cicd/docker/Dockerfile.runtime:31`, `Dockerfile:149`).
- Outbound path: `EffectiveCa::System` maps to `webpki_roots::TLS_SERVER_ROOTS` in
  `link_sys_core/tls.rs:105` (Redis/Kafka/LinkSys rustls), `tls/validation/outbound_client.rs:31` (OIDC/LDAP
  SAN-verified clients), `otlp/transport.rs:62`, `ext_proc/client.rs:462` and `ldap_auth/plugin.rs:219`.
  The workspace `reqwest` is built with `rustls-tls-webpki-roots` only (`Cargo.toml:135`), so every
  `build_outbound_client` consumer also trusts the compiled-in bundle and not the OS store.
- Docs make the split explicit only for LinkSys ("built-in webpki roots",
  `skills/02-features/03-resources/18-link-sys.md:97`). BackendTLSPolicy documentation describes `System` as
  the system trust store.

## Industry comparison
- Gateway API `BackendTLSPolicy.validation.wellKnownCACertificates: System` is defined as the
  implementation's system trust store. Envoy Gateway and Istio map it to the proxy image's OS bundle (or an
  operator-provided bundle file). Every egress path uses that one bundle.
- Kong uses a single `lua_ssl_trusted_certificate` (default `system`) for all Lua-initiated TLS. NGINX
  `proxy_ssl_trusted_certificate` / `lua_ssl_trusted_certificate` are likewise one operator-chosen file.

## Impact
- A common practice is to add a corporate or private root to the image or node trust store (or mount a CA
  bundle at the OS path). With that, BackendTLSPolicy backends validate, but OIDC discovery, Webhook
  key-resolution, ext_proc, LDAP, OTLP, ES and Kafka with `System` fail TLS verification for the same issuer.
  Operators must then duplicate the CA into a Secret reference on every resource.
- Root-store updates (distrusts, new roots) for outbound clients need a Gateway rebuild, while backends
  follow the image's `ca-certificates` package. Security response and "which roots do we trust" audits
  must track two sources.

## Suggested direction
Pick one canonical meaning of `System` in the shared `outbound_tls` resolver and use it on both paths:
- **Option A (recommended)**: `System` means the OS trust store for everything. Load it once at startup
  (`rustls-native-certs`, or the same file BoringSSL uses) into a process-wide `RootCertStore` that every
  `EffectiveCa::System` consumer reuses. Expose an optional process-config override
  (`gateway.system_ca_bundle_path`) that both BoringSSL (`ServerConf.ca_file`) and the rustls clients honor.
- Option B: `System` means the compiled-in webpki bundle for everything. Set the Pingora connector's
  `ca_file` to an exported copy of the same bundle so that backends do not read OS paths.
Option A matches Gateway API and operator expectations. Whichever is chosen, document it once in the
outbound TLS contract instead of per resource.

## Notes
Checked `skills/04-review/tls/` (`backend-ca-bundle-multi-pem.md`: behavior with `use_system_ca=false`
only), `skills/04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md` (notes "explicit
webpki System roots" for OTLP), and the LinkSys feature doc. Each page documents its own path. None records
a decision that the two meanings should differ. `arch-04` (FIPS build leaves control/egress TLS on ring)
concerns the crypto provider, not trust-anchor selection.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P3 unchanged.
- Backend path: `backends/peer_builder.rs:170` installs a custom CA only when `!use_system_ca`. The Pingora fork connector calls `set_default_verify_paths()` (`pingora-core/src/connectors/tls/boringssl_openssl/mod.rs:115,139`), which reads the OS store.
- Outbound path: `EffectiveCa::System` maps to `webpki_roots::TLS_SERVER_ROOTS` in `edgion-common/src/link_sys_core/tls.rs:109-111` and `edgion-gateway/src/tls/validation/outbound_client.rs:29-31`. `reqwest` is built with `rustls-tls-webpki-roots` only (`Cargo.toml:135`).
- No review page records the split as intended. This is distinct from arch-04, which is about the crypto provider.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P3 unchanged.
- `edgion-common/src/link_sys_core/tls.rs` was rewritten for reqwest 0.13.5 (`Cargo.toml:136`, `rustls-no-provider`), but `EffectiveCa::System` still maps to `webpki_roots::TLS_SERVER_ROOTS` (`tls.rs:104-106`), as do `tls/validation/outbound_client.rs:29-31`, `otlp/transport.rs:62`, `ext_proc/client.rs:462` and `ldap_auth/plugin.rs:219`.
- Backend path unchanged: `backends/peer_builder.rs:170` installs a CA only when `!use_system_ca`, leaving Pingora's `set_default_verify_paths()` OS store.
- `link-sys/regression-guards/shared-outbound-tls-loading.md` now states outbound HTTP (and Redis, after the Fred→redis-rs move) uses public WebPKI roots to "preserve Edgion's existing trust contract"; it documents the outbound side only and does not reconcile it with the backend OS-store meaning, so the split remains undecided. Redis default trust moved from native to WebPKI, widening the WebPKI side.
