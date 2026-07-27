---
name: pingora-fork-branch-lock-policy
description: Use when reviewing Edgion Pingora Git dependency pins, Cargo.lock reproducibility, release tags, or --locked CI and image builds.
---

# Pingora Fork Branch and Lockfile Policy

## Decision

The six direct `pingora-*-edgion` workspace dependencies form one indivisible
source family.

- Normal development selects `branch = "edgion"` for all six entries.
- `Cargo.lock` records the exact resolved commit for the complete transitive
  fork family.
- CI, release workflows, Cargo Chef, and image builds use `--locked`.
- A release milestone may replace the branch selector with one protected
  immutable tag, but all six entries must switch together.
- Per-package `rev` pins are not used. They make every fork advance a
  six-location manifest edit without improving a locked build's
  reproducibility.

The stock comparison family remains the exact crates.io Pingora `0.8.1`
packages and must never resolve through the fork URL.

## Why a movable branch remains reproducible

Cargo's Git source selector and resolved commit are separate lockfile facts.
The manifest says which branch to follow when an intentional dependency update
is performed; the committed lockfile records the commit selected at that time.
`--locked` refuses to rewrite the lockfile, so a later branch movement cannot
change CI or a release build.

An intentional fork advance therefore consists of:

1. refreshing the lockfile without `--locked`;
2. reviewing that only the Pingora source family moved;
3. running the source-policy guard and both Gateway flavor builds;
4. committing the manifest/lockfile state together.

## Enforcement

`cicd/checks/check_pingora_source_policy.py` verifies:

- all six direct aliases point to the expected fork and package names;
- every entry uses the same supported selector (`edgion` branch or one release
  tag), with no `rev`;
- every fork package in `Cargo.lock` uses that selector;
- all locked fork packages resolve to exactly one commit;
- all six direct packages exist in the locked fork family.

`deny.toml` allows only the Pingora Git repository. Its generic
`required-git-spec = "any"` is intentionally broader than the project policy
because cargo-deny cannot express “branch in development or tag for release.”
The custom guard supplies that narrower selector rule and the single-commit
lock invariant.

The CI source-isolation tree check remains separate: it proves the body binary
contains only fork Pingora packages and the stock binary contains only
crates.io Pingora packages.
