---
name: h2-ci-contract-enforcement-out-of-scope
description: Historical review evidence for Pingora fork / H2 CI contract enforcement; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 ci contract enforcement out of scope

## Recorded conclusion

Enforcing the H2 security contracts and an `h2` minimum/current dependency matrix is a CI/CD change (workflow YAML plus dependency-resolution config) landing in the Pingora fork repository, not Edgion, and touches no production code, so it is not handled through the review-issue workflow. A blanket "no H2 test may stay `#[ignore]`d" guard is also wrong today: of the sixteen `#[ignore]` markers under `pingora-core/src/protocols/http/v2/`, eight in `client.rs` are blocked on the upstream `h2` decoder fix (H2-001/H2-004) and seven in `end_stream_watch.rs` are negative characterizations that must never be enabled. Deleting or enabling those markers, moving enforcement into Edgion's CI, and closing H2-001/H2-004 on the same reasoning are not accepted. At this finding's closure the then-declared `h2 = ">=0.4.16"` minimum leg remained untested by design; the current fork baseline is `>=0.4.19`.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-ci-contract-enforcement-out-of-scope.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-ci-contract-enforcement-out-of-scope.md)

## Resolution references

- `H2-009` — 2026-08-28 (wont-fix at the review layer; CI/CD scope).
