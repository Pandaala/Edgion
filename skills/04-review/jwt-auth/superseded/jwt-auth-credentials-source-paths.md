---
name: jwt-auth-credentials-source-paths
description: Inactive four-path credential-loading decision, replaced by immutable prebuilt issuer groups.
knowledge_type: superseded
---

# Historical JWT four-path credential loading

Inactive: P2-12 was closed as a design tradeoff on 2026-04-26 for the former
`load_credentials_from_secret` implementation. Its four resolved/fallback and
single/multi branches and `RwLock<Option<Credentials>>` no longer exist. The
historical decision against extracting helpers cannot suppress current findings.

The current `JwtIssuerGroupStore::new` consumes Controller-resolved issuer groups
and prepares `DecodingKey` values before requests. There is no runtime Secret
fetch. See `edgion-gateway/src/plugins/http/jwt_auth/credentials.rs`, the
[prebuilt-key guard](../regression-guards/decoding-key-prebuilt.md), and the
[issuer/key binding contract](../mandatory-design/issuer-key-binding.md).

D0036 source inspection confirmed the owning implementation was replaced. Current
copy reductions must preserve configured algorithm, scoped key ownership, and
failure classification; the removed schema paths provide no compatibility
obligation. No implementation change or new benchmark follows from this correction.
