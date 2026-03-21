# CI and Release Workflow Guide

This document is for contributors who maintain GitHub Actions, publish Docker images, or need to reproduce CI behavior locally. It explains the current Edgion CI / release structure, the shared local action, command boundaries, and the highest-risk change points.

> The primary AI / agent workflow now lives in [../../../skills/06-cicd/02-github-workflow.md](../../../skills/06-cicd/02-github-workflow.md).
> This document remains the human-facing background guide, release-flow explanation, and manual review checklist.

## The Two Main Workflow Families

The repository currently has two main GitHub Actions paths.

### 1. CI checks

File:

- `.github/workflows/ci.yml`

Triggers:

- push to `main` / `master`
- pull requests targeting `main` / `master`

Responsibilities:

- basic compilation checks
- formatting checks
- agent-doc / skills / dev-guide entry validation
- clippy
- the `cargo test --all` test entry used in CI

### 2. Release and image publishing

File:

- `.github/workflows/build-image.yml`

Trigger:

- push tag `v*`

Responsibilities:

- prefetch Cargo dependencies
- build Linux binaries for `amd64` and `arm64`
- build and push runtime images
- merge multi-arch manifests

## Shared Infrastructure: `setup-rust`

Both CI and release now depend on the same local action:

- `.github/actions/setup-rust/action.yml`

That action currently owns three concerns:

1. installing the Rust toolchain, optional components, and optional targets
2. installing Ubuntu build dependencies when needed
3. restoring and saving Cargo caches

The goal is straightforward:

- avoid two separate piles of Rust setup logic in `ci.yml` and `build-image.yml`
- make toolchain, dependency, and cache changes in one place

So if you edit this action, treat it as shared infrastructure rather than a private helper for one job.

## What CI Currently Runs

The current `ci.yml` check order is:

1. `cargo check --all-targets`
2. `cargo fmt --all -- --check`
3. `make check-agent-docs`
4. `cargo clippy --all-targets`
5. `cargo test --all`
6. summary job `ci-success`

A few details are easy to misunderstand:

- the current CI commands do not automatically expand to `--all-features`
- `make check-agent-docs` is the guardrail for `AGENTS.md`, `skills/`, and the dev-guide entry layer
- the `fmt` job does not install system dependencies and does not use Cargo cache
- `ci-success` exists to provide a stable branch-protection result, not to re-run checks

## How Release Currently Flows

`build-image.yml` currently breaks down into four stages.

### 1. `prepare-cargo-cache`

Purpose:

- checkout
- call `setup-rust`
- cache `~/.cargo/registry` and `~/.cargo/git`
- run `cargo fetch`

### 2. `build-binaries`

Purpose:

- build a matrix for `amd64` and `arm64`
- produce three binaries:
  - `edgion-gateway`
  - `edgion-controller`
  - `edgion-ctl`
- install the cross compiler for arm64

### 3. `build-and-push-images`

Purpose:

- download the binary artifacts from the previous stage
- build runtime images via `docker/Dockerfile.runtime`
- push architecture-specific tags

### 4. `merge-manifests`

Purpose:

- extract the version from the `vX.Y.Z` tag
- merge `amd64` and `arm64` images
- generate:
  - full version tag
  - minor tag
  - major tag
  - `latest`

## How Local Commands Relate To Workflows

For this repository, command ownership works best in layers:

- repository-wide common commands live in `AGENTS.md`
- workflow-specific operational detail lives in skills
- human-facing explanation stays in `docs/`

If you only want to reproduce the current CI checks locally, start with:

```bash
cargo check --all-targets
cargo fmt --all -- --check
make check-agent-docs
cargo clippy --all-targets
cargo test --all
```

If you need something closer to release binary packaging, start with:

- `skills/06-cicd/00-local-build.md`
- `skills/06-cicd/01-docker-build.md`
- `skills/06-cicd/02-github-workflow.md`

## The Most Common Pitfalls When Editing Workflows

### 1. Workflow changed, shared action not updated

Examples:

- adding a new target or component that `setup-rust` cannot express
- changing cache behavior in a workflow while the local action still uses the old paths

### 2. Binary build path changed, image packaging not updated

Artifact output paths, upload/download steps, and `Dockerfile.runtime` form one chain. Changing only one segment breaks release.

### 3. Tag rules changed, manifest merge logic not updated

`merge-manifests` currently assumes `vX.Y.Z`. If release tagging changes, image-tag derivation must be reviewed too.

### 4. Mixing “convenient local commands” with “mandatory CI commands”

Local debugging commands can be more flexible. CI commands should stay stable, cost-aware, and reproducible. Do not expand CI just because a heavier command happened to help one local debugging session.

## Manual Review Checklist

- do triggers still match the intended branch / tag policy
- does `.github/actions/setup-rust/action.yml` still cover all workflow inputs
- do cache keys still avoid cross-job pollution while keeping reuse
- does release still publish only `gateway` and `controller` runtime images
- are git tags, image tags, and manifest tags still semantically aligned
- do local reproduction commands still match what workflows actually run

## If You Want AI To Help

Start from the skill entry:

- [../../../skills/06-cicd/02-github-workflow.md](../../../skills/06-cicd/02-github-workflow.md)

If the task also touches feature combinations or build dependencies, continue with:

- [../../../skills/02-development/06-feature-flags.md](../../../skills/02-development/06-feature-flags.md)
- [../../../skills/06-cicd/00-local-build.md](../../../skills/06-cicd/00-local-build.md)
- [../../../skills/06-cicd/01-docker-build.md](../../../skills/06-cicd/01-docker-build.md)

## Related Docs

- [AI Collaboration and Skill Usage Guide](./ai-agent-collaboration.md)
- [Knowledge Source Map and Maintenance Rules](./knowledge-source-map.md)
- [Makefile](../../../Makefile)
