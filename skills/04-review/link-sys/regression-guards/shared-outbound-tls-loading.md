---
name: shared-outbound-tls-loading
description: Historical review evidence for LinkSys / TLS; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Shared outbound tls loading

## Recorded conclusion

`link_sys::tls::apply_to_reqwest_builder` centralises CA root + mTLS identity loading for all reqwest-based providers (ES / webhook / new providers); call sequence is `apply_insecure_skip_verify` → `apply_to_reqwest_builder`; duplication-across-providers findings closed

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `g9a-007` — —.
