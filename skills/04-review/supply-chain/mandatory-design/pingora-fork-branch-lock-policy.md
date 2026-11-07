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

- Normal development selects `branch = "edgion_v0.9.1"` for all seven entries.
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
- every entry uses the same supported selector (`edgion_v0.9.1` branch or one release
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

## Synchronization evidence boundaries

Source consistency, upstream ancestry, and behavioral acceptance are separate
checks. A passing source-policy guard establishes one locked fork family; it
does not establish that a synchronization test matrix passed or that an old
published head has an immutable remote reference. Record the actual consumer
HEAD, locked source, remote branch observation, and local fork changes separately.
Inspect resolved Cargo metadata as well as the manifest to exclude a local
override from consumer verification.

### Current lock: `edgion_v0.9.1` rebase

The current lockfile resolves all 16 fork packages to
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`, and `h2` to 0.4.20.
Lock history on `edgion_0.9.0_v1`: `0b0d8bba9609655bfd7db0fe0add6818bc0ee937`
(Edgion `c633ef4a7`, 2026-10-08), then `1aaa45cc70430bc782990bc20ddee3fef36d6b99`
(`963d2664a`, 2026-11-03; `0b0d8bba` plus six fork commits), then the current lock
(`e270a928e`, 2026-11-05). The new branch is not a fast-forward of either: the fork
patch series was rebased and partly rewritten onto upstream 0.9.1
(`f86ffda5d3ae5c3baf5125682138b33b26c01b08`), so neither `0b0d8bba` nor `1aaa45cc`
is an ancestor of `e5f4287`. Compare the series with
`git range-diff 702f690..1aaa45cc f86ffda..e5f4287be713` in the fork repository.

Consequence for review knowledge (`arch-05`): a conclusion verified against fork or
`h2` internals at `0b0d8bba` is historical evidence, not current evidence, until it is
re-verified at the current lock. On 2026-11-06 the `skills/04-review` pages that cited
`0b0d8bba` as the locked revision were re-verified by source inspection at `e5f4287`
and updated (many of them had last been verified at `0b0d8bba`, not at the
immediate predecessor `1aaa45cc`); pages that still mention `0b0d8bba` or `edgion_0.9.0_v1` do so only as
explicitly historical evidence. Recorded test results on those pages still belong to
`0b0d8bba`. The same day's follow-up closed three residual gaps: the h2 0.4.20
private-fact audit of the end-stream watcher (recorded on
`h2-grpc/regression-guards/h2-end-stream-wire-observer.md`), Edgion response-cache
truncation/trailer non-admission evidence (on
`http-body/regression-guards/bounded-response-cache-capture.md`), and the
re-verification at `e5f4287` of the pages outside the `arch-05` set that still called
`1aaa45cc`, `518f7274`, or `h2` 0.4.19 current. Still open, in
`tasks/todo/pingora-v0.9.1-rebase-evidence-followups.md`: unpublished fork shutdown
tests, stale fork-side records, and fresh fork-suite runs at `e5f4287`. The Tokio
exact pin was lifted to a `1.54.1` floor the same day (see
`supply-chain/compatibility-constraints/pingora-tokio-zero-capacity.md`). Repeat this sweep
whenever the lockfile moves the fork family: search `skills/04-review` for every fork
commit and `h2` version other than the current lock (not only the immediate
predecessor), and keep any remaining mention only as explicitly historical evidence.

### Historical: P090-002 synchronization (`edgion_0.9.0_v1`)

P090-002's scoped investigation on 2026-10-05 confirmed that consumer
`d05d95576d8d046b3221b1d8e444123fc85e7f89` locks all 16 fork packages to
`0b0d8bba9609655bfd7db0fe0add6818bc0ee937`. The adopted upstream milestone
`702f69015e53f7244d6ad2e743de571d859a70a4` is its ancestor. The observed
remote branch head was `ce7b8e805fa34d7bc0240a7253a254b9b55582a1`; its
later changes are not implicitly selected or validated by the consumer lock.
Normal consumer metadata resolved 15 active fork packages, all from that Git
revision's Cargo checkout; the complete lock family still contains 16.

The fork's stale candidate/publication prose is being corrected separately
from historical snapshots. Its local uncommitted test corrections cannot
turn the locked revision's failing tests into passing results. Reusing a
Cargo target directory with a source archive's old timestamps can also reuse
stale local-crate artifacts: require demonstrable recompilation before using
such an archive as verification evidence.

The original synchronization task (`P090-002`) recorded verified local test
corrections and consumer acceptance, with publication provenance still open at
that time. Its task file is absent from the current tree; absence does not prove
that publication completed. Candidate success is not evidence that its
uncommitted test changes entered the locked source or a completed release.
Re-evaluate when the consumer lock, tested worktree,
dependency resolution, or remote publication evidence changes.

## Resolution references

- The user authorized updating the development selector from `edgion_v5` to
  `edgion_0.9.0_v1` to match the existing manifest and lockfile. URL, selector
  consistency, no-rev and single-commit enforcement remain unchanged.
- `BODY-WORK-008` — 2026-07-27.
- `P090-002` — scoped documentation/provenance correction and locked-baseline
  verification; the original task file is no longer available in this tree.
