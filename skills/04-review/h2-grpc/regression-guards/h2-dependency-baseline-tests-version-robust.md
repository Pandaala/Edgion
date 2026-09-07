---
name: h2-dependency-baseline-tests-version-robust
description: Historical review evidence for Pingora fork / H2 dependency baseline tests; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 dependency baseline tests version robust

## Recorded conclusion

The unwatched "laundering" baselines in `client.rs` characterize the resolved `h2` release, not a Pingora contract, so they accept both the laundered and the rejected outcome and report which one they saw via `report_dependency_baseline`. With the then-declared open-ended `h2 = ">=0.4.16"` range, requiring the laundering made an upstream fix arrive as a red CI run that reads like a product regression; the current fork baseline is `>=0.4.19` and retains the same behavior-based test design. The fail-closed contract stays strict: the `h2_watched_*` siblings assert it under either dependency, and wherever a baseline sees a body-read error it still requires `read_trailers()` to keep failing. The direct trailer read answering `Ok(None)` under a rejecting `h2` is classified and reported rather than failed, because latching it is H2-004's deferred work. Restoring the hard laundering assertion, deleting the baselines, pinning `h2` to an exact version, asserting on a version instead of behavior, and widening the shared `assert_clean_response` helper are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-dependency-baseline-tests-version-robust.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-dependency-baseline-tests-version-robust.md)

## Resolution references

- `H2-011` — 2026-08-28 (fixed in pingora fork `edgion_v3`).
