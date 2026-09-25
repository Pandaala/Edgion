---
name: pingora-fork-branch-lock-policy
description: Use when reviewing Edgion Pingora Git dependency pins, Cargo.lock reproducibility, release tags, or --locked CI and image builds.
knowledge_type: mandatory-design
---

# Pingora Fork Branch and Lockfile Policy

## Decision

The seven direct fork workspace dependencies (`pingora-core`,
`pingora-http`, `pingora-proxy`, `pingora-limits`, `pingora-load-balancing`,
`pingora-ketama`, and `TinyUFO`) form one indivisible source family, sourced exclusively from
the Edgion fork (`Pandaala/pingora`). There is no stock crates.io Pingora
family — the former `body-buffering`/`upstream-pingora` flavor duality was
removed on 2026-08-03 (fork-only build).

- Normal development selects `branch = "edgion_0.9.0_v1"` for all seven entries.
- `Cargo.lock` records the exact resolved commit for the complete transitive
  fork family.
- CI, release workflows, Cargo Chef, and image builds use `--locked`.
- A release milestone may replace the branch selector with one protected
  immutable tag, but all seven entries must switch together.
- Per-package `rev` pins are not used. They make every fork advance a
  seven-location manifest edit without improving a locked build's
  reproducibility.

## Why a movable branch remains reproducible

Cargo's Git source selector and resolved commit are separate lockfile facts.
The manifest says which branch to follow when an intentional dependency update
is performed; the committed lockfile records the commit selected at that time.
`--locked` refuses to rewrite the lockfile, so a later branch movement cannot
change CI or a release build.

An intentional fork advance therefore consists of:

1. refreshing the lockfile without `--locked`;
2. reviewing that only the Pingora source family moved;
3. running the source-policy guard and the Gateway build + test suite;
4. committing the manifest/lockfile state together.

## Enforcement

`cicd/checks/check_pingora_source_policy.py` verifies:

- all seven direct dependencies point to the fork URL;
- every entry uses the same supported selector (`edgion_0.9.0_v1` branch or one release
  tag), with no `rev`;
- every fork package in `Cargo.lock` uses that selector;
- all locked fork packages resolve to exactly one commit;
- all seven direct packages exist in the locked fork family.

`deny.toml` allows only the Pingora Git repository. Its generic
`required-git-spec = "any"` is intentionally broader than the project policy
because cargo-deny cannot express “branch in development or tag for release.”
The custom guard supplies that narrower selector rule and the single-commit
lock invariant.

The CI source-isolation tree check remains separate: it proves the Gateway
binary contains only fork Pingora packages (no registry-sourced Pingora may
enter the dependency graph).

## Resolution references

- The user authorized updating the development selector from `edgion_v5` to
  `edgion_0.9.0_v1` to match the existing manifest and lockfile. URL, selector
  consistency, no-rev and single-commit enforcement remain unchanged.
- `BODY-WORK-008` — 2026-07-27.
