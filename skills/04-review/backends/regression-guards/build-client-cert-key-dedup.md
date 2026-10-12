---
name: build-client-cert-key-dedup
description: Native policy and Gateway fallback identities share one generation-time PEM parser.
knowledge_type: regression-guards
---

# One native backend identity parser

The historical backends-F008 correction (2026-05-24) removed duplicate policy
and Gateway identity parsing. ARCH-017 moves the remaining canonical
`build_client_cert_key_from_secret` to
`edgion-gateway/src/backends/prepared_backend_tls.rs`. Both publication owners
call it once when native material changes; peer construction clones its outcome.
There is no production forwarding helper or request-time PEM parser.

The parser preserves full certificate chains, missing/empty/malformed-material
`Unknown` failures, and existing private-key decoding. It does not introduce
certificate/key matching validation. Failed outcomes remain present and are
checked before custom trust errors. Existing protected identity parser and pool
isolation tests retain their test identities and mismatched-key hash fixtures.

The former `build_client_cert_key` and peer-local helper descriptions are historical
source shapes. Re-evaluate duplication and publication ownership when new native
identity sources or validation requirements are introduced. General Common rustls
outbound clients and independent active-health transport keep their own boundaries.
Fresh verification is recorded in the active task; this page does not reuse the
historical correction date as evidence of current test execution.
