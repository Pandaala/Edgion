---
name: gateway-tls-backends
description: Gateway TLS backend boundary — BoringSSL is supported, rustls is a dormant future-development seam, and FIPS is an opt-in BoringSSL image feature.
---

# TLS build backends and capability boundaries

## Cargo features

`edgion-gateway` exposes two mutually exclusive TLS backend features, but only
`boringssl` is a supported Gateway product/build target:

```toml
default = ["allocator-jemalloc", "boringssl"]
boringssl = ["pingora-core/boringssl", "pingora-proxy/boringssl", "dep:boring-sys", "dep:boring"]
rustls = ["pingora-core/rustls", "pingora-proxy/rustls"]
fips = ["boringssl", "boring/fips"]
```

There is no gateway `openssl` feature or `tls/openssl` implementation. Do not introduce an
OpenSSL fallback branch when changing TLS code.

The `rustls` feature is retained only as a future-development seam. It has no
clean-build, CI, Docker, integration-test, or runtime capability contract.

## Capability boundary

| Capability | BoringSSL build | rustls feature |
|------------|-----------------|--------------|
| Gateway build/release/CI contract | Supported | Unsupported; a clean build is not guaranteed |
| Plain HTTP and non-terminating data-plane paths | Supported | No product contract |
| Custom HTTPS downstream termination and dynamic certificates | Supported | Unsupported |
| TLSRoute terminate-to-TCP | Supported | Unsupported |
| TLSRoute passthrough | Does not terminate TLS | No product contract |
| EdgionTls SNI selection, per-connection minimum version/ciphers, mTLS allowlists, SSL ex_data | Supported by the BoringSSL-gated runtime | Unsupported |
| Upstream BackendTLSPolicy | Supported | No parity contract |
| Image-only BoringSSL FIPS mode | `fips` feature | Unsupported |

The custom runtime is gated in `tls/runtime/mod.rs` and `tls/mod.rs`. Use
BoringSSL for every Gateway deployment. Do not treat rustls-only compile errors,
warnings, missing plugin portability, incomplete TLS behavior, Docker/CI gaps,
or test-harness gaps as current findings. Future work is consolidated in
`tasks/block/rustls-gateway-tls-support.md`. Edgion will not implement Gateway
rustls support until Pingora provides complete rustls support for the required
downstream and upstream TLS surface, verified against the revision Edgion uses.
A compiling feature or partial API availability is insufficient. After that gate
is met, implementation still requires explicit user authorization and a complete
product design. Backend-independent uses of the `rustls` library are unaffected.

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

When enforcement is active and both `EdgionTls.spec.ciphers` and
`EdgionGatewayConfig.spec.downstreamTls.ciphers` are absent, downstream TLS defaults to the
approved TLS 1.2 cipher set in `fips_profile::FIPS_TLS12_CIPHERS` and the NIST P-curves P-256,
P-384, and P-521. EdgionTls overrides EGC ciphers; explicit `[]` selects backend defaults. An explicit operator cipher list wins for TLS 1.2; the curve restriction remains
active whenever FIPS policy is enforced. An explicit downstream curve list may
select an ordered subset of those NIST curves; a list containing X25519 rejects
the handshake. Curve application belongs to the early ingress policy callback,
independent of certificate-source and cipher overrides.

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
