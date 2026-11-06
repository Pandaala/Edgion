# arch-04: FIPS build covers only downstream/Pingora-upstream TLS; conf_sync and all plugin egress TLS use non-FIPS `ring`

- **Category**: design-error
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `Cargo.toml:102` `tonic = { version = "0.14.9", features = ["tls-ring", ...] }` — gateway conf_sync client, ext_proc, OTLP
  - `Cargo.toml:136` `reqwest` with `rustls-no-provider`; `build_outbound_client` falls back to `apply_default_http_tls` (`edgion-common/src/link_sys_core/tls.rs:170,187`, explicit `ring::default_provider()`)
  - `Cargo.toml:153` `ldap3` with `tls-rustls-ring`
  - `Cargo.toml:149,150,154` `redis` / `etcd-client` / `rskafka` TLS (rustls)
  - `edgion-common/src/startup.rs:117-118` process-wide `ring::default_provider().install_default()`
  - `edgion-gateway/Cargo.toml:45` `fips = ["boringssl", "boring/fips"]`

## Summary

The `fips` feature switches BoringSSL into FIPS mode, and `require_fips` / `fips_policy` gate startup and
downstream cipher policy. But the Gateway process performs a large amount of TLS through rustls with the
`ring` provider, which is not a FIPS-validated module: the conf_sync mTLS channel that carries resolved
secrets, every plugin/LinkSys egress (OIDC token/JWKS, forward_auth, ext_proc, webhooks, LDAP, Redis, Kafka,
etcd, OTLP, Elasticsearch), and AI provider calls made through the shared outbound client. A FIPS image
therefore passes `require_fips: true` while its most security-sensitive outbound channels use non-approved
crypto. This gap is not listed among the documented FIPS limitations (which mention only TLS 1.3 suites and
Pingora upstream ciphers).

## Current behavior (evidence)

- `fips` touches only `boring` (`edgion-gateway/Cargo.toml:45`); no rustls provider is switched.
- Workspace TLS stacks: tonic 0.12 `tls` (rustls/ring), reqwest `rustls-tls-*` (ring), ldap3
  `tls-rustls-ring`, tonic 0.14 `tls-ring`; none is FIPS-selectable today.
- Docs (`skills/02-features/01-binary-and-deployment/02-feature-flags.md`,
  `skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md`) describe FIPS as BoringSSL
  scope only, without stating that conf_sync and egress are outside it.

## Industry comparison

- **Envoy FIPS builds** (`--define boringssl=fips`): all TLS — downstream, upstream, xDS gRPC, ext_authz,
  ext_proc, OTLP — goes through one BoringSSL-FIPS module.
- **Istio FIPS (Tetrate/Solo distributions)**: istiod and proxies are both built against BoringCrypto, including
  the xDS channel.
- **Go-based gateways** (Traefik/Kong Go plugins): use `GOEXPERIMENT=boringcrypto`/Go FIPS module process-wide.

## Impact

- Operators enabling FIPS for regulatory reasons get a false sense of coverage; an auditor checking the
  crypto inventory will find ring in the TLS path of credential and secret transport.
- The CLAUDE.md caveat ("NOT a compliance claim") is necessary but the specific non-FIPS channels should not be
  invisible.

## Suggested direction

1. Short term: document precisely which TLS channels are inside/outside the FIPS module (conf_sync, egress
   clients) in the FIPS feature docs and the startup WARN when `require_fips` is true.
2. Medium term: under `fips`, switch rustls consumers to the `aws-lc-rs` provider with its `fips` feature
   (rustls 0.23 `CryptoProvider` installed process-wide at startup, `reqwest`/`tonic` via
   `rustls` provider configuration), and fail startup if any client builds with a non-FIPS provider; or move
   egress TLS onto BoringSSL connectors where a crate supports it.
3. Add a CI guard that a `--features fips` build has no `ring` TLS provider in its rustls configuration
   (e.g. `cargo tree -e features -i ring` check).

## Notes

- Not related to the blocked Gateway rustls *downstream* backend task (`tasks/block/rustls-gateway-tls-support.md`);
  this is about backend-independent outbound clients, which CLAUDE.md explicitly says are unaffected by that block.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-common/src/startup.rs:117-118`: `rustls::crypto::ring::default_provider().install_default()` — the process-wide rustls provider is ring in every build, including `--features fips`.
- `edgion-gateway/src/plugins/http/ext_proc/client.rs:448`, `ldap_auth/plugin.rs:196`, `tls/validation/outbound_client.rs:22`: explicit `ring::default_provider()` client configs.
- `edgion-gateway/Cargo.toml:45`: `fips = ["boringssl", "boring/fips"]` touches only boring; `Cargo.toml` rustls has both `aws_lc_rs` and `ring` features but nothing selects aws-lc FIPS.
- `skills/02-features/01-binary-and-deployment/02-feature-flags.md` FIPS section describes BoringSSL scope only; conf_sync and egress rustls channels are not listed as limitations.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-common/src/startup.rs:117-118`: process-wide rustls provider is still `ring` in every build; `edgion-gateway/Cargo.toml:45` `fips` still touches only `boring`.
- Dependency upgrades did not change the picture: tonic 0.14 uses `tls-ring` (`Cargo.toml:102`); reqwest 0.13 moved to `rustls-no-provider` but `build_outbound_client` now applies `apply_default_http_tls`, which builds an explicit `ring` `ClientConfig` (`link_sys_core/tls.rs:170`); explicit ring configs remain in `ext_proc/client.rs:448`, `ldap_auth/plugin.rs:196`, `tls/validation/outbound_client.rs:22`.
- `skills/02-features/01-binary-and-deployment/02-feature-flags.md:50-51` and the rewritten `skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md` still describe FIPS as BoringSSL-only without listing conf_sync/egress rustls channels as outside scope.
