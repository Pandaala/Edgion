---
name: per-request-connection-accepted
description: Use when reviewing LDAP per-request connection or pooling findings after credential-result caching was removed.
knowledge_type: intentional-design
---

# LDAP Auth Per-Request Connection — Accepted with Operational Bounds

`ldap_auth` performs a fresh LDAP connection and `simple_bind` for every
credential-bearing request. Authentication results are intentionally not
cached, so credential revocation and directory policy changes are checked by
the authoritative LDAP service on every request.

Do not introduce a generic credential cache as a performance fix. LDAP
`simple_bind` changes the connection's authenticated identity, so pooling or
reusing connections across users requires a separate security design that
proves identity reset, serialization, failure cleanup, and bounded concurrency.

Operators should size the LDAP service for the protected request rate and use
the existing `RateLimitLocal` plugin to bound abusive traffic. Re-evaluate this
decision if production evidence shows connection exhaustion, or if the LDAP
client (`edgion-ldap`: bind, unbind, StartTLS and single-entry search; no
pooling) gains a pool with first-class per-bind identity-reset semantics. Search
mode keeps one per-request connection for the service bind, the search and the
user bind (three round trips; with group requirements a successful login adds
a service re-bind and a base-scope group search, five in all); a pooled service-account connection is a separate
design, not a performance fix for this page.

Source: `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`.

## Resolution references

- `plugins-http-FB04` — 2026-05-27 (⚠️ accepted tradeoff).
