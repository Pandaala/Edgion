---
name: body-suite-sweep-registration
description: Historical review evidence for Integration / body-suite sweep registration; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Body suite sweep registration

## Recorded conclusion

Fixed: `DslBody` and `BodyPlugins` remain registered in resource/default sweeps and resource-wide config loading, with guards for suite counts and the skip logging path. The stock-flavor skip semantics became obsolete on 2026-08-03 when the stock Pingora flavor was removed (fork-only build); the canon doc was retired.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `BODY-005` — 2026-07-27.
