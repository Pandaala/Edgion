---
name: gateway-tls-backends
description: Gateway TLS build backends and capability boundaries — BoringSSL, rustls, and the opt-in FIPS image feature.
---

# TLS build backends and capability boundaries

## Cargo features

`edgion-gateway` has two mutually exclusive TLS backend features:

```toml
default = ["allocator-jemalloc", "boringssl"]
boringssl = ["pingora-core/boringssl", "pingora-proxy/boringssl", "dep:boring-sys", "dep:boring"]
rustls = ["pingora-core/rustls", "pingora-proxy/rustls"]
fips = ["boringssl", "boring/fips"]
```

There is no gateway `openssl` feature or `tls/openssl` implementation. Do not introduce an
OpenSSL fallback branch when changing TLS code.

## Capability boundary

| Capability | BoringSSL build | rustls build |
|------------|-----------------|--------------|
| Plain HTTP and non-terminating data-plane paths | Supported | Supported where Pingora exposes the path |
| Custom HTTPS downstream termination callbacks | Supported | Not implemented; listener construction fails explicitly |
| TLSRoute terminate-to-TCP | Supported | Not implemented because it uses the same custom callback runtime |
| TLSRoute passthrough | Does not terminate TLS | Does not require the custom termination callback |
| EdgionTls SNI selection, per-connection minimum version/ciphers, mTLS allowlists, SSL ex_data | Supported by the BoringSSL-gated runtime | Not implemented by the gateway callback runtime |
| Image-only BoringSSL FIPS mode | `fips` feature | Not available |

The custom runtime is gated in `tls/runtime/mod.rs` and `tls/mod.rs`. Treat build-backend support
and feature-level support as separate questions: a crate may compile with rustls while a particular
listener type still rejects startup.

## BoringSSL implementation

The BoringSSL path owns:

- the select-certificate, certificate, and handshake-complete callback chain;
- SSL ex_data lifecycle and early-abort audit logging;
- downstream mTLS certificate-chain and SAN/CN allowlist verification;
- per-connection minimum TLS version and TLS 1.2 cipher configuration;
- backend certificate SAN extraction/hooks used by upstream policy enforcement.

TLS 1.3 cipher suites cannot be restricted through BoringSSL's TLS 1.2 cipher-list API.

## FIPS build and policy

`fips` is an explicit image-build feature, not a runtime Cargo default and not
part of the ordinary release workflow. Boring 5 builds its bundled FIPS module
source in dedicated Linux amd64 and arm64 builders, which then run the isolated
module and TLS-profile verification. The FIPS-only source override pins Boring
5.2.0's BoringSSL revision plus the upstream arm64 integrity-relocation fix;
ordinary builders do not fetch or use that override. At startup:

- `require_fips: true` fails startup when BoringSSL FIPS mode is not present;
- `fips_policy` is `auto | on | off` and resolves with environment (`EDGION_FIPS_POLICY`) over
  CLI (`--fips-policy`) over config over `auto`;
- `auto` follows `/proc/sys/crypto/fips_enabled` and tightens the downstream profile on a FIPS host;
- a non-FIPS binary on a FIPS host emits a warning and exposes host/module metrics.

When enforcement is active and `EdgionTls.spec.ciphers` is absent, downstream TLS defaults to the
approved TLS 1.2 cipher set in `fips_profile::FIPS_TLS12_CIPHERS` and the NIST P-curves P-256,
P-384, and P-521. An explicit operator cipher list wins for TLS 1.2; the curve restriction remains
active on a genuine FIPS build.

Limitations:

- BoringSSL TLS 1.3 suites cannot be restricted, so ChaCha20 can still negotiate.
- Upstream gateway-to-backend cipher suites are not FIPS-restricted by this policy.
- The runtime FIPS-mode signal and host-policy alignment do not establish CMVP
  certificate scope, artifact provenance, or deployment compliance.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/Cargo.toml` | TLS backend and FIPS features |
| `edgion-gateway/src/tls/mod.rs` | TLS module feature gates |
| `edgion-gateway/src/tls/runtime/mod.rs` | BoringSSL-only custom runtime boundary |
| `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` | Downstream callback implementation |
| `edgion-gateway/src/tls/runtime/backend/` | BoringSSL backend certificate helpers |
| `edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs` | mTLS allowlist callback |
| `edgion-gateway/src/tls/fips_profile.rs` | Approved profile and tri-state policy resolution |
| `edgion-gateway/src/tls/fips_host.rs` | Host-kernel FIPS detection |
