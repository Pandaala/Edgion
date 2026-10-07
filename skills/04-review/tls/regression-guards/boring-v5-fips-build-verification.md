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
FEATURES="allocator-jemalloc,fips" \
  ./cicd/build-image.sh --arch amd64,arm64 --compile-only
```

The original finding assumed the ordinary release workflow promised a FIPS
artifact and therefore required a separate precompiled-artifact provenance
contract. That is not the product boundary: FIPS is an explicitly requested
Edgion build, and the ordinary release remains non-FIPS.

Boring 5 builds its bundled FIPS module source when the `fips` feature is
selected. Boring 5.2.0 vendors BoringSSL revision
`e2a57cfb4d915b4ba820585aef9fdee7bca13fe5`, whose AArch64 `delocate` pass omits
`BORINGSSL_bcm_text_hash` from its synthesized-symbol set. A GOT relocation can
then point at the module start rather than the hash object, so the constructor
compares the module's first 32 bytes with its calculated integrity HMAC and
aborts before Rust `main`.

The canonical Edgion FIPS builders support amd64 and arm64. They pin that same
BoringSSL revision and checksum-verify the exact patch from upstream commit
`4d6b2f971e456962b70e4e91768caffdb36fe016`, which synthesizes the hash symbol's
local target. The override has a separate builder tag and is only supplied via
`BORING_BSSL_FIPS_SOURCE_PATH` when the exact `fips` feature is selected.
The base revision, fix revision, and patch checksum are encoded into that path,
so changing any source input also changes Cargo's build-script environment
cache key and forces `boring-sys` to rebuild. Ordinary builders do not fetch
it. `BORING_BSSL_FIPS_PATH` remains an upstream prebuilt-tree escape hatch
outside this script contract.

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

## Intentional boundaries

- The ordinary release workflow does not enable `fips`.
- The source override is FIPS-only; ordinary amd64 and arm64 builds remain on
  Boring's bundled source and ordinary release builds remain non-FIPS.
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

## Re-evaluation triggers

- Boring changes the `fips` feature or its source-build prerequisites.
- A released `boring-sys` vendors upstream commit
  `4d6b2f971e456962b70e4e91768caffdb36fe016` or a descendant, allowing the
  FIPS-only source override to be removed.
- Edgion decides to publish a dedicated FIPS image in the release workflow.
- The integration suite gains a separate FIPS-compatible compatibility profile.

## References

- `cicd/build-image.sh`
- `cicd/docker/Dockerfile.builder`
- `tasks/block/boring-upstream-arm64-fips-source-override-removal.md`
- `edgion-tests/integration/scripts/integration/test_fips_policy.sh`
- `docs/en/ops-guide/gateway/tls/fips.md`

## Resolution references

- `FIPS-01` — 2026-08-30.
