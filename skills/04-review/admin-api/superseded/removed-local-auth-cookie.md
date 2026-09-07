---
name: removed-local-auth-cookie
description: Historical review evidence for Security / Local-auth cookie Secure; revalidate against current code before applying.
knowledge_type: superseded
---

# Removed local auth cookie

## Recorded conclusion

**Obsolete (2026-06-12): the local-auth cookie subsystem was deleted when Admin API auth went token-only (Bearer header only, no cookie).** Historical record: the local-auth `edgion_token` cookie `Secure` attribute was made configurable via `LocalAuthConfig.cookie_secure` (default `true`) so the JWT cookie no longer replayed over plaintext `http://`. There is no longer any login/logout handler or session cookie — the token-only model carries the bearer token in the `Authorization` header only. Do not re-file.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `common-center-02` — 2026-05-24.
