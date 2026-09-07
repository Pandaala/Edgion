---
name: ldap-auth-cache-timing-oracle
description: Historical LDAP cache-timing finding; no longer applicable because LDAP credential-result caching was removed.
knowledge_type: intentional-design
---

# LDAP Auth Cache Timing — Resolved by Cache Removal

LDAP authentication no longer caches successful credentials. The former
cache-hit versus LDAP-bind timing path, SHA-256 credential cache key, and
`cacheTtl` operator setting do not exist.

Do not reintroduce credential-result caching or cache-hit jitter under this
historical finding. Any future LDAP cache requires a new security design that
covers credential revocation, policy freshness, memory bounds, and timing.

## Resolution references

- `F-012` — 2026-05-21 (⚠️ accepted tradeoff).
