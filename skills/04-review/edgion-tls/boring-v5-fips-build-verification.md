---
name: boring-v5-fips-build-verification
description: Use when reviewing Edgion's Boring 5 FIPS build contract. FIPS is an explicit build-image.sh feature, not part of the ordinary release workflow; the canonical builder compiles the bundled module source and verifies the module signal plus the approved TLS 1.2 profile.
---

# Boring 5 FIPS opt-in build and verification

**Status:** fixed · 2026-08-29

## Conclusion

Edgion supports a FIPS-enabled Gateway as an explicit Docker build, while the
ordinary release workflow and default builds remain non-FIPS. The canonical
entry point is:

```bash
FEATURES="allocator-jemalloc,fips" \
  ./cicd/build-image.sh --arch amd64 --compile-only
```

The original finding assumed the ordinary release workflow promised a FIPS
artifact and therefore required a separate precompiled-artifact provenance
contract. That is not the product boundary: FIPS is an explicitly requested
Edgion build, and the ordinary release remains non-FIPS.

Boring 5 builds its bundled FIPS module source when the `fips` feature is
selected. The canonical Edgion path supports amd64 only. It rejects arm64
before compilation because BoringSSL's own runtime integrity self-test fails
for that source build. `BORING_BSSL_FIPS_PATH` is an upstream prebuilt-tree
escape hatch outside this script contract; Edgion does not impose a separate
artifact format.

## Verification contract

For amd64, `cicd/build-image.sh` runs the existing isolated
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
- Ordinary non-FIPS arm64 builds remain supported; only the canonical source
  FIPS build is amd64-only.
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
- BoringSSL's arm64 source FIPS integrity self-test becomes supported upstream.
- Edgion decides to publish a dedicated FIPS image in the release workflow.
- The integration suite gains a separate FIPS-compatible compatibility profile.

## References

- `cicd/build-image.sh`
- `cicd/docker/Dockerfile.builder`
- `edgion-tests/integration/scripts/integration/test_fips_policy.sh`
- `docs/en/ops-guide/gateway/tls/fips.md`
