# Remove the BoringSSL ARM64 FIPS Source Override

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | blocked |
| Type | dependency/build |
| Priority | P2 |
| Issue | FIPS-01 follow-up |

## AI guide summary

Remove Edgion's FIPS-only BoringSSL source override after an official
`boring-sys` release vendors BoringSSL commit
`4d6b2f971e456962b70e4e91768caffdb36fe016` or a descendant containing its
`BORINGSSL_bcm_text_hash` synthesized-symbol fix.

## Why this remains open

`boring-sys 5.2.0` vendors BoringSSL revision
`e2a57cfb4d915b4ba820585aef9fdee7bca13fe5`. On AArch64 FIPS builds, that
revision can leave the integrity hash behind a GOT relocation which resolves to
the module start, causing BoringSSL's constructor self-test to abort. Edgion's
explicit FIPS builder currently pins the same revision and applies the exact,
checksum-verified upstream fix. This is intentionally a temporary dependency
workaround, not an Edgion fork.

## Trigger

As of 2026-09-08, the official registry still lists 5.2.0 as the latest
non-yanked release of both `boring` and `boring-sys`. The published
`boring-sys 5.2.0` source still omits `BORINGSSL_bcm_text_hash` from
`isSynthesized`. The user deferred removal until the release condition below
is met. Keep the existing override; this task remains unresolved.

A non-yanked `boring` and `boring-sys` release used by both Edgion and the
Edgion Pingora fork vendors the upstream fix.

## Acceptance criteria

- Confirm the released crate's vendored BoringSSL ancestry contains
  `4d6b2f971e456962b70e4e91768caffdb36fe016` or an equivalent reviewed fix.
- Update the shared `boring` and `boring-sys` dependency without splitting the
  Pingora/Edgion Boring type identity.
- Remove the FIPS source checkout, patch download, checksum, source-path
  environment variable and cache fingerprint, FIPS-only builder tag, and this
  TODO reference.
- Run the canonical explicit FIPS build and isolated runtime/TLS verification
  on both amd64 and arm64.
- Confirm an ordinary non-FIPS builder no longer contains or downloads a
  separate BoringSSL source tree.
