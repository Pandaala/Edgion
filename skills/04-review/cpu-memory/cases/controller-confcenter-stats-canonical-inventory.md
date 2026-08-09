---
name: controller-confcenter-stats-canonical-inventory
description: Use when reviewing controller cache-residue coverage for newly registered resource kinds or wipe-all tier drift.
---

# Controller ConfCenter stats use the canonical resource inventory

## Decision

`collect_conf_center_counts` iterates `ALL_RESOURCE_INFOS`. It must not maintain a hand-written
subset of `ResourceKind` values. Every registered resource is therefore visible to the
`cache_residue` validator through `conf_center_storage.<Kind>`.

The wipe path remains tiered because deletion order matters, but its coverage test compares the
union of all tiers with `ALL_RESOURCE_INFOS` and separately rejects duplicates. Adding a resource
therefore requires choosing a deletion tier before the test passes.

## Required tests

- An empty ConfCenter reports a zero entry for every item in `ALL_RESOURCE_INFOS`.
- The wipe-tier union exactly equals the canonical inventory and contains no duplicates.

These tests protect different invariants: visibility of residue and actual deletion coverage.

## Rejected alternatives

- Append only the currently missing kinds to a fixed store-stats array: the next resource can drift
  again without a compile or test failure.
- Assert only a numeric kind count: a duplicate can hide an omission, and the expected number must
  be manually bumped.
- Reuse wipe-tier order as the stats inventory: leak visibility should cover the canonical resource
  registry even while a newly added kind is waiting for an explicit tier choice.
