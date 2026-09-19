---
name: rustls-tls13-encryption-level-validation
description: Use when reviewing rustls lockfile downgrades or RUSTSEC-2026-0285 findings involving TLS 1.3 handshake encryption-level validation.
knowledge_type: regression-guards
---

# Rustls TLS 1.3 Encryption-Level Validation

## Conclusion and scope

Edgion's workspace lockfile must resolve `rustls` to `0.23.45` or later. Versions
from `0.23.13` through `0.23.44` are affected by RUSTSEC-2026-0285: they can
accept a TLS 1.3 handshake message at the wrong encryption level instead of
terminating the connection with `unexpected_message`.

This boundary covers every workspace consumer of the shared `rustls 0.23`
dependency, including backend-independent outbound clients and infrastructure
TLS. It does not add support for the dormant Gateway rustls server backend;
BoringSSL remains the only supported Gateway TLS backend.

## Rationale and evidence

Before the correction, `Cargo.lock` resolved `rustls 0.23.43`. A fresh
`cargo deny check advisories` reported RUSTSEC-2026-0285 and showed production
dependency paths through `edgion-common`, `edgion-controller`,
`edgion-gateway`, and `edgion-cli`, in addition to `edgion-tests`.

The workspace manifest already allowed compatible `0.23.x` releases, so the
root cause was the stale resolved version rather than an API or feature
constraint. Updating the lockfile to `rustls 0.23.45` also resolved its
`rustls-webpki` dependency to `0.103.15`; no manifest, feature, or application
code change was required.

## Alternatives and limits

Adding RUSTSEC-2026-0285 to the advisory ignore list was rejected because a
compatible patched release was available. A repository-level reproduction of
the malformed record is not maintained: the defect and correction are inside
the upstream TLS state machine, while Edgion's regression boundary is the
resolved version plus the advisory gate.

The correction does not prove that every future `rustls` release is free from
security defects. New advisories require fresh assessment, and an intentional
downgrade must remain outside the affected range.

## Verification and re-evaluation

Verified on 2026-09-19:

- `cargo deny check advisories` passed with `rustls 0.23.45`;
- `cargo test -p edgion-common --lib grpc_tls` passed 5 tests;
- `cargo test -p edgion-common --lib link_sys_core::tls` passed 16 tests;
- `cargo test -p edgion-gateway --lib tls::validation::outbound_client` passed
  4 tests.

Re-evaluate if the workspace changes rustls release lines, removes the shared
dependency, or a later advisory supersedes this patched-version boundary.

## Resolution references

- `RUSTSEC-2026-0285` — fixed on 2026-09-19 by resolving `rustls 0.23.45`;
  discovered by Gateway quality campaign deferred issue `D0002`.
