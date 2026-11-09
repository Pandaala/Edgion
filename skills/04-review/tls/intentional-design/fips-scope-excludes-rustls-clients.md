---
name: fips-scope-excludes-rustls-clients
description: Use when reviewing whether the Gateway `fips` build covers conf_sync, LinkSys, plugin, AI-provider, or other rustls client TLS; those channels stay on the rustls `ring` provider by accepted product scope.
knowledge_type: intentional-design
---

# FIPS build scope excludes rustls client channels

## Conclusion and scope

The Gateway `fips` feature (`edgion-gateway/Cargo.toml`: `fips = ["boringssl", "boring/fips"]`)
places only BoringSSL in FIPS mode. That covers Pingora downstream TLS and Pingora
upstream (Gateway -> backend) TLS. Every TLS channel built on rustls stays on the
`ring` crypto provider in every build, including `--features fips`:

- the process-wide provider installed by `edgion_common::startup::init_crypto`;
- the gateway conf_sync gRPC mTLS client and other tonic clients (`tls-ring`), such as
  ext_proc and OTLP;
- `build_outbound_client` default TLS (`edgion-common/src/link_sys_core/tls.rs`), and
  therefore OIDC/JWKS, forward_auth, webhook, Elasticsearch, AI provider and other
  shared outbound HTTP calls;
- explicit `ring` client configs in `ext_proc/client.rs`, `ldap_auth/plugin.rs`
  (handed to `tokio-rustls`), and `tls/validation/outbound_client.rs`;
- Redis, etcd and Kafka LinkSys TLS through the process-wide rustls provider.

Switching these clients to a FIPS-validated provider (for example `aws-lc-rs` with
its `fips` feature) is **not current product scope**. The user decided on
2026-11-06 not to pursue rustls FIPS provider support for now and to close
`arch-04` without code or documentation changes.

This is distinct from the blocked Pingora rustls Gateway *backend*
([rustls-gateway-backend-unsupported](rustls-gateway-backend-unsupported.md)):
these are backend-independent rustls clients that exist in every build.

## Rationale and evidence

- `edgion-common/src/startup.rs` `init_crypto` installs
  `rustls::crypto::ring::default_provider()` unconditionally.
- Workspace `Cargo.toml`: tonic `tls-ring`, reqwest
  `rustls-no-provider` with an explicit `ring` `ClientConfig` in
  `apply_default_http_tls`.
- No `fips`-gated code selects another rustls provider.

- The workspace `rustls` dependency enables both `aws_lc_rs` and `ring`, so non-FIPS
  AWS-LC is already linked (also via `jsonwebtoken`), but nothing installs or selects
  an AWS-LC FIPS provider.

Benefit of the current choice: one installed rustls provider across all binaries and
tests, no AWS-LC *FIPS* module (`aws-lc-fips-sys`) built beside BoringSSL-FIPS, and no
extra FIPS build prerequisites for the rustls dependency tree.

Accepted cost and risk: a FIPS image that passes `require_fips: true` still performs
conf_sync (which carries resolved secrets) and all plugin/LinkSys egress TLS with
non-validated `ring` crypto. An auditor reviewing the crypto inventory will find
`ring` on those paths. The FIPS operator guide's "Known limitations" section does
not list these channels; this was raised and deliberately left unchanged in this
decision. The FIPS build is already documented as not a compliance claim.

Rejected alternatives (for now):

- Installing an `aws-lc-rs` FIPS provider under `fips` and replacing every explicit
  `ring` config, plus the tonic feature switch.
- Moving egress TLS onto BoringSSL connectors.
- A CI guard that a `--features fips` build has no `ring` rustls provider.

## Verification and re-evaluation

Verification on 2026-11-06 was source inspection only (provider install site, the
explicit `ring` config sites listed above, and workspace dependency features). No
FIPS build or runtime crypto-inventory test was run for this decision.

Re-evaluate when any of the following occurs:

- the user requests FIPS coverage of conf_sync or egress TLS, or a customer/auditor
  requirement names those channels;
- Edgion publishes a dedicated FIPS image in the release workflow;
- the `fips` feature definition changes, or the process-wide rustls provider changes;
- a decision is made to document the excluded channels in the FIPS operator guide.

## Resolution references

- `arch-04` (fips-build-leaves-control-and-egress-tls-on-ring) — confirmed and
  re-verified on feature-10-28; closed 2026-11-06 as an accepted product-scope
  decision without code change.
- Related build contract: [Boring 5 FIPS opt-in build and verification](../regression-guards/boring-v5-fips-build-verification.md).
