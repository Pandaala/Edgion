---
name: supply-chain-review
description: Use when reviewing dependency upgrades, new crate additions, MSRV bumps, license/maintenance-mode crate risks, or lockfile audit findings; documents the `cargo audit`/`cargo deny` cadence and triage rules.
---

# Supply Chain / Dependencies Review

> Decision notes and triage rules for Edgion's Rust dependency supply chain.

## Threat Model

- **Malicious / compromised upstream crates**: a crate maintainer (or attacker with
  publish rights) ships a malicious version. Mitigated by lockfile pinning + `cargo audit`.
- **Abandoned / maintenance-mode crates**: maintainer stops responding; security
  advisories go unfixed. Risk is latent until a CVE surfaces. See
  [maintenance-mode-crate-acceptance.md](maintenance-mode-crate-acceptance.md) for
  the established acceptance criteria.
- **Vulnerable transitive dependency**: a direct dep's dep has a known advisory.
  Triage using `cargo audit` — may require either a direct upgrade or a `[patch]` override.
- **MSRV vs CI matrix mismatch**: a new crate raises the effective minimum supported
  Rust version above what CI validates. Check `Cargo.toml` `rust-version` field and
  the CI matrix before merging.
- **License risk**: a new crate introduces a copyleft or non-commercial license
  incompatible with Edgion's distribution model. Use `cargo deny` to enforce the
  license allowlist.

## Tooling

| Tool | Purpose | When to run |
|------|---------|-------------|
| `cargo audit` | Check `Cargo.lock` against the RUSTSEC advisory database | Manually (not yet CI-gated — see Cadence) |
| `cargo deny` | License allowlist, source allowlist, banned crates | Manually (no `deny.toml` checked in yet) |
| Manual lockfile review | Verify no unexpected version bumps or new transitive deps | Before merging a `Cargo.lock`-touching PR |

## Cadence

> **Reality check (verified 2026-05-29)**: `cargo audit` and `cargo deny` are **NOT**
> currently wired into CI. The `.github/workflows/` set is `ci.yml`, `check-english-only.yml`,
> `build-image.yml`, `docs.yml` — none invoke audit/deny, and there is no `deny.toml` in the
> repo root. So supply-chain advisories and license violations are **not** gated on merge today;
> they rely on manual runs. Wiring these into `ci.yml` (and adding a `deny.toml`) is an open
> hardening gap — do not assume an existing gate when reviewing.

- **Per PR (intended, not yet enforced)**: run `cargo audit` and `cargo deny` manually on
  `Cargo.lock`-touching PRs until they are added to CI.
- **Pre-release**: full manual lockfile diff against the previous release tag; triage any new advisories or new crates.
- **Quarterly manual triage**: review maintenance state of crates flagged as "feature-complete / low activity" — check for new advisories, successor projects, or changed maintainer status.

## Triage Rules

| Severity | Action |
|----------|--------|
| RUSTSEC advisory marked **critical** or **high** | Block merge; upgrade or patch before landing |
| RUSTSEC advisory marked **medium** | Triage within one sprint; document decision in this directory if deferring |
| RUSTSEC advisory marked **low** / informational | Log in this directory; re-evaluate at next quarterly triage |
| Advisory for an **unused** code path | Confirm the path is unreachable (e.g., feature gate), document, accept with re-evaluation trigger |
| Maintenance-mode crate (feature-complete, low activity) | Apply the acceptance criteria in [maintenance-mode-crate-acceptance.md](maintenance-mode-crate-acceptance.md) |
| New crate added without `cargo deny` license check | Block; add the crate to `deny.toml` allowlist only after license verification |

## Known Rules in This Directory

| File | Topic |
|------|-------|
| [maintenance-mode-crate-acceptance.md](maintenance-mode-crate-acceptance.md) | Criteria for accepting feature-complete / low-activity crates; based on `kstring` evaluation |
| [pingora-fork-branch-lock-policy.md](pingora-fork-branch-lock-policy.md) | Edgion Pingora branch/tag selector, lockfile, and `--locked` build policy |

## Usage

When a PR adds or upgrades a crate:

1. Run `cargo audit` and `cargo deny` locally.
2. Check the crate's MSRV against the CI matrix.
3. Review the crate's maintenance state if it is new to the dependency graph.
4. If deferring a known advisory or accepting a maintenance-mode crate, add a rule file to this directory.
