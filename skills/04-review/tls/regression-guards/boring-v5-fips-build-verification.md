---
name: boring-v5-fips-build-verification
description: Use when reviewing Edgion's Boring 5 FIPS build contract. FIPS is an explicit build-image.sh feature, not part of the ordinary release workflow; the canonical builder compiles the bundled module source and verifies the module signal plus the approved TLS 1.2 profile.
knowledge_type: regression-guards
---

# Boring 5 FIPS opt-in build and verification

**Status:** fixed · 2026-08-30

## Conclusion

Edgion supports a FIPS-enabled Gateway as an explicit Docker build, while the
ordinary release workflow and default builds remain non-FIPS. The canonical
entry point is:

```bash
FEATURES="allocator-jemalloc,fips,full" \
  ./cicd/build-image.sh --arch amd64,arm64 --compile-only
```

The original finding assumed the ordinary release workflow promised a FIPS
artifact and therefore required a separate precompiled-artifact provenance
contract. That is not the product boundary: FIPS is an explicitly requested
Edgion build, and the ordinary release remains non-FIPS.

Boring 5.4 builds its bundled FIPS module source when the `fips` feature is
selected. The published `boring-sys 5.4.0` vendors BoringSSL revision
`d3bf4795ad5b79d646cddf2fec687da5b9b502db`, a descendant of the upstream
arm64 integrity-hash relocation fix
`4d6b2f971e456962b70e4e91768caffdb36fe016`. Its `delocate` implementation
registers `BORINGSSL_bcm_text_hash` in the symbols map and emits its local target.
Both halves of the fix are present in the published crate, not only the Git tree.

Previously, Boring 5.2.0 bundled revision
`e2a57cfb4d915b4ba820585aef9fdee7bca13fe5`, whose AArch64 relocation could
point at the module start rather than its hash object and abort before Rust
`main`. Edgion's former FIPS-only builder pinned that revision and applied the
checksum-verified upstream fix. The current builders need no separate source
checkout, patch, source environment override or FIPS-only builder tag. Ordinary
and FIPS builds share the architecture builder and use the bundled source.
`BORING_BSSL_FIPS_PATH` remains an upstream prebuilt-tree escape hatch outside
the canonical script contract.

## Verification contract

For each selected architecture, `cicd/build-image.sh` runs the existing isolated
`test_fips_policy.sh` scenario against the release binary before
packaging succeeds. The scenario:

- starts the Gateway with `EDGION_REQUIRE_FIPS=1`, proving a non-FIPS binary
  cannot satisfy the check;
- requires `edgion_gateway_fips_module_enabled` to report `1`;
- rejects a ChaCha20-only TLS 1.2 client; and
- accepts an approved AES-GCM TLS 1.2 client.

The builder includes Go and Clang because BoringSSL's source FIPS build generates
the `delocate` and `inject_hash` tools during compilation. It also includes the
runtime utilities used by the isolated scenario.

## Current verification

The 2026-11-02 dependency maintenance run passed the canonical ordinary and
explicit FIPS builds on Linux arm64 and amd64 using bundled Boring 5.4.0 source.
Both ordinary module-0 policy probes passed; both FIPS binaries started with
`EDGION_REQUIRE_FIPS=1`, reported module 1, rejected ChaCha20 and accepted approved
AES-GCM under TLS 1.2. The obsolete source-override follow-up is closed.

One shared ordinary Linux topology also passed TLS, gRPC TLS, SNI, mTLS, cipher,
SAN, curve-reload and last-known-good certificate scenarios, with strict final
resource/cache cleanup. Native TLS unit checks passed. The macOS live runner
stopped before startup because required loopback aliases were absent; no macOS
live result is claimed.

## Intentional boundaries

- The ordinary release workflow does not enable `fips`.
- Both ordinary and FIPS builds use Boring's bundled source. Ordinary release
  builds remain non-FIPS.
- The full compatibility integration suite does not run under the FIPS binary.
  Several cases intentionally require weak protocol or cipher support; turning
  that run into a skip list would weaken both signals. The isolated scenario is
  the dedicated FIPS contract.
- A package-local `build.rs` does not preflight BoringSSL inputs. Cargo may run
  dependency build scripts first, and Boring 5 already owns its source/prebuilt
  selection and validation.
- The runtime module signal and TLS profile check prove FIPS mode and the built
  behavior. They do not identify a CMVP validated-module stream, certificate
  scope, artifact provenance, or the compliance of a complete platform or
  deployment.
- `fips` covers BoringSSL only. Rustls client channels (conf_sync, LinkSys,
  plugin and AI-provider egress) stay on `ring`; see
  [FIPS build scope excludes rustls client channels](../intentional-design/fips-scope-excludes-rustls-clients.md).

## Re-evaluation triggers

### TLS 1.3 cipher limitation and resumption

The cipher-list API restriction applies to both full and resumed TLS 1.3
handshakes. Do not explain it as PSK skipping Edgion's certificate callback:
in the historically inspected boring-sys 5.2.0, `ssl/handshake_server.cc::do_cert_callback` invokes
`cert_cb` before entering the TLS 1.3 state machine, and
`ssl/tls13_server.cc` selects session reuse later. The cipher-list contract in
`include/openssl/ssl.h` explicitly excludes TLS 1.3 from these setters.

Downstream curve policy is applied separately in the early ClientHello
callback. This source verification does not establish PSK-specific curve
negotiation coverage; no such live test is claimed. Recheck this distinction
when changing the TLS backend, callback order, cipher APIs, or resumption tests.

`CAND-B-002` corrected the misleading PSK explanation in both language versions
of the FIPS operator guide. It was a documentation error, not evidence of a
runtime PSK policy bypass.

### Build contract

- Boring changes the `fips` feature or its source-build prerequisites.
- Boring changes the bundled source or the arm64 integrity-relocation fix.
- The Gateway adopts a TLS 1.3 cipher-selection API; the current policy uses
  TLS 1.2 cipher-list setters only.
- Edgion decides to publish a dedicated FIPS image in the release workflow.
- The integration suite gains a separate FIPS-compatible compatibility profile.

## References

- `cicd/build-image.sh`
- `cicd/docker/Dockerfile.builder`
- `edgion-tests/integration/scripts/integration/test_fips_policy.sh`
- `docs/en/ops-guide/gateway/tls/fips.md`

## Resolution references

- `FIPS-01` — 2026-08-30.

## Downstream default precedence

The early `DownstreamTlsPolicy` owner applies EdgionTls explicit ciphers over EGC
`downstreamTls.ciphers`, then the active FIPS default. Explicit empty lists select
backend defaults at either level. Curve policy is EGC-only and still rejects
X25519 under active FIPS, even with a cipher override. Certificate callbacks no
longer apply a second FIPS profile. Minimum versions inherit EGC defaults with
local EdgionTls overrides. Both language operator guides describe these axes.

`CAND-DOC-001` corrected the detailed cipher decision trees in both operator
guides: they must include the EGC fallback and explicit-empty semantics, just
like the introductory precedence text. With local ciphers absent and EGC set
to `[]` or an explicit expression, jumping directly to FIPS defaults predicts
the wrong cipher policy. Check these examples against `CipherPolicy::resolve`
and `apply`; curve restrictions remain independent. This is a documentation
correction, not a runtime policy change or new FIPS verification claim.
