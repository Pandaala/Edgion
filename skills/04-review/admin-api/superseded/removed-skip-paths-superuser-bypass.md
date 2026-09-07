---
name: removed-skip-paths-superuser-bypass
description: Historical review evidence for Security / auth / operator `skip_paths` Superuser bypass; revalidate against current code before applying.
knowledge_type: superseded
---

# Removed skip paths superuser bypass

## Recorded conclusion

**Obsolete (2026-06-12): the entire subsystem this finding concerned was deleted when Admin API auth went token-only.** Historical record: operator-supplied `[auth].skip_paths` / `[local_auth].skip_paths` entries were merged into the `unified_auth` skip-set with no allowlist; a business route added there bypassed `unified_auth`, and on the Controller `assign_superuser_if_no_role` Case 3 then granted `Superuser` (unauthenticated full admin). It was fixed by a `skip_paths` allowlist + a claims-gate on the superuser assignment. None of that code exists anymore — there is no `skip_paths`, no `unified_auth`, no `Role::Superuser`; the token-only model has a fixed static-public set and no role-assignment fallback. Do not re-file against the old model.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `fr-cauth-01` — 2026-05-29.
