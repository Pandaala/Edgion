---
name: direct-bind-identity
description: Use when reviewing the username forwarded after LDAP authentication.
knowledge_type: intentional-design
---

# Identity forwarded after a direct bind

For `ldap-auth-follow-ups` item 7, the accepted choice was to document the
identity source for each mode. A direct bind verifies credentials without an
entry read. In the Gateway LDAP plugin, `run_exchange` returns `None` on a
successful direct bind, and `run_request` writes the trimmed credential username
to `credentialIdentifierHeader`, preserving its case.

A directory can accept multiple spellings for the same entry. An upstream ACL
keyed on the stored spelling can therefore miss a match. Search mode already
returns the entry's selected `attribute` after successful authentication.
Reading it after every direct bind would add a directory round trip and require
the authenticated user to have read access. Revisit if deployments need the
stored identity but cannot use the service-account search flow.
