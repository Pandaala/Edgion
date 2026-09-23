---
name: github-workflow
description: Use when changing GitHub Actions CI, release tags, Docker image publishing, or the shared setup-rust local action used by Edgion workflows.
---
# GitHub Workflow

Use this skill to answer:

- Which jobs, commands, and caches GitHub CI currently runs
- After a tag is published, how Docker images are built per architecture and merged into a manifest
- How to modify the `setup-rust` local action without simultaneously breaking `ci.yml` and `build-image.yml`
- How to reproduce the CI / release build path locally as closely as possible

## Workflow Inventory

The repo currently has the following workflows under `.github/workflows/`:

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | `push` / `pull_request` to `main` / `master` | Rust check / fmt / clippy / test + agent-docs gate |
| `waf-production-acceptance.yml` | `push` / `pull_request` to `main` / `master`, manual | Strict selected WAF profile report and Gateway wire/hot-reload acceptance |
| `check-english-only.yml` | `pull_request` + `push` to `main` | Runs `./cicd/checks/check_english_only.sh` — the English-only gate the project relies on |
| `check-gateway-no-tracing.yml` | `pull_request` + `push` to `main` | Runs `./cicd/checks/check_gateway_no_tracing.py` — forbids unapproved `tracing::*` macros on data-plane hot paths (function-level) |
| `check-ssa-force.yml` | repository workflow trigger | Ensures every server-side apply uses forced `PatchParams`, subject to the script allowlist |
| `check-edgion-config-data-overlay.yml` | `pull_request` + `push` to `main` | Three gates guarding the EdgionConfigData overlay refactor — see "EdgionConfigData overlay gates" below |
| `supply-chain.yml` | `pull_request` + `push` to `main` + daily cron | `cargo deny check` against `deny.toml` (RustSec advisories, license allowlist, banned crates, source pinning). Cron runs advisories-only so new advisories surface without waiting for a PR; triage decisions live as commented `ignore` entries in `deny.toml`. Do NOT pass `--all-features` (TLS/allocator features are mutually exclusive) |
| `docs.yml` | `push` to `main` touching `docs/**` (or `workflow_dispatch`) | VitePress build + GitHub Pages deploy |
| `build-image.yml` | `workflow_dispatch` (manual only) | Cross-compile binaries, build/push runtime images, merge manifests, create release |

### EdgionConfigData overlay gates
`check-edgion-config-data-overlay.yml` runs three structural checks (each in its own job for clean failure reporting):

1. `cicd/checks/check_no_legacy_data_refs.sh` — forbids reintroduction of removed types/kinds/fields (`EdgionConfigData` kind, `ClusterRegionRoute` / `ServiceRegionRoute` discriminators, `serviceRegionRouteRef` / `clusterRef` fields, removed Rust identifiers). Exclusions: `tasks/`, `cicd/`, `tmp/`. Counterpart to D1 / D4 / I3 in the refactor task.
2. `cicd/checks/check_edgion_config_data_variant_parity.py` — diffs the EdgionConfigData CRD `type:` enum against the Rust `ConfigEntry` variant set. Catches the S3-style drift where Rust variants are added/removed but the CRD enum is forgotten.
3. `cicd/checks/check_overlay_signal_completeness.sh` — every overlay-consumer plugin must emit the full four-signal set (`overlay_missing`, `overlay_type_mismatch`, `overlay_invalid`, `overlay_disabled`) via its Prometheus counter. Catches the H1-style omission where one signal is silently missing.

## Read these real entry points first

- [../../../.github/workflows/ci.yml](../../../.github/workflows/ci.yml)
- [../../../.github/workflows/build-image.yml](../../../.github/workflows/build-image.yml)
- [../../../.github/workflows/check-english-only.yml](../../../.github/workflows/check-english-only.yml)
- [../../../.github/workflows/check-ssa-force.yml](../../../.github/workflows/check-ssa-force.yml)
- [../../../.github/workflows/check-gateway-no-tracing.yml](../../../.github/workflows/check-gateway-no-tracing.yml)
- [../../../.github/workflows/docs.yml](../../../.github/workflows/docs.yml)
- [../../../.github/actions/setup-rust/action.yml](../../../.github/actions/setup-rust/action.yml)
- [../../../cicd/docker/Dockerfile.runtime](../../../cicd/docker/Dockerfile.runtime)
- [00-local-build.md](00-local-build.md)
- [01-docker-build.md](01-docker-build.md)

## Current CI pipeline

`ci.yml` triggers on `push` / `pull_request` to `main` or `master`, in the following order:

1. `check` -> Pingora source-policy guard, then
   `cargo check --locked --workspace --all-targets`
2. `gateway-build` -> build the Gateway artifact, probe its compiled
   capabilities, and verify every Pingora package resolves to the Edgion fork
3. `fmt` -> `cargo fmt --all -- --check`
4. `agent-docs` -> `python3 cicd/checks/validate_agent_docs.py`
5. `clippy` -> `cargo clippy --locked --workspace --all-targets`
6. `test` -> `cargo test --locked --all`
7. `ci-success` -> aggregates the results of the previous six jobs to provide a stable check for branch protection

Key facts:

- The five Rust jobs `check`, `gateway-build`, `fmt`, `clippy`, and `test`
  each run the local action
  [../../../.github/actions/setup-rust/action.yml](../../../.github/actions/setup-rust/action.yml)
  first
- `agent-docs` only needs checkout + `python3 cicd/checks/validate_agent_docs.py`; it does not depend on Rust toolchain initialization
- `fmt` does not install system dependencies and does not enable cargo cache;
  the other build-style Rust jobs install dependencies and enable cache
- The current CI commands do not include `--all-features`; before changing this, confirm whether you really want to expand the matrix rather than casually inflate CI cost
- Every resolving Cargo command uses `--locked`. The manifest follows the
  Edgion Pingora `edgion_v5` branch, but CI always builds the exact commit in the
  committed lockfile. `check_pingora_source_policy.py` also verifies that all
  seven direct fork dependencies share that branch and the complete lockfile
  family resolves to one commit.
- `check` and `clippy` pass `--workspace` on purpose: `edgion-tests` is a workspace member excluded from `default-members`, so a bare `--all-targets` would skip the harness. `--workspace` gives it the same fast type-check + clippy signal that `cargo test --all` already exercises at compile time

## What the local `setup-rust` action does

This action is the shared front layer used by both CI and release. It currently has only three responsibilities:

1. Install Ubuntu build dependencies on demand:
   - `build-essential`
   - `cmake`
   - `pkg-config`
   - `protobuf-compiler`
   - `libssl-dev`
   - `libclang-dev`
   - `clang`
2. Restore / save the Cargo cache
3. Install the rustup toolchain, optional components, and optional targets

When changing it, treat it as "shared infrastructure", not as a private script for some single job:

- When changing the cache path or key, consider both `ci.yml` and `build-image.yml`
- When changing the dependency list, verify [../../../edgion-common/build.rs](../../../edgion-common/build.rs) and the Docker builder dependencies remain consistent
- When changing the toolchain/component/target inputs, ensure `fmt`, `clippy`, and cross-compilation can still express what they need

## Current release workflow

`build-image.yml` currently triggers on **`workflow_dispatch` only** (manual run). The `push: tags: - "v*"` trigger is commented out in the workflow file, so pushing a `v*` tag no longer auto-builds images or auto-creates a release. To restore auto-release on tag push, uncomment that `push.tags` block. The workflow is split into 5 jobs:

1. `prepare-cargo-cache`
   - checkout
   - call `setup-rust`
   - cache only `~/.cargo/registry` and `~/.cargo/git`
   - `cargo fetch --locked`

2. `build-binaries`
   - `amd64` / `arm64` matrix
   - arm64 additionally installs `gcc-aarch64-linux-gnu` / `g++-aarch64-linux-gnu`
   - Builds three binaries (the release build step does **not** include `edgion-center`):
     - `edgion-gateway`
     - `edgion-controller`
     - `edgion-cli`
   - Uploads the artifact `edgion-binaries-<arch>`

3. `build-and-push-images`
   - Downloads the binaries from the previous stage
   - Builds runtime images using [../../../cicd/docker/Dockerfile.runtime](../../../cicd/docker/Dockerfile.runtime)
   - Pushes per architecture (only `gateway` and `controller` get images; `edgion-cli` is built but not imaged):
     - `pandaala/edgion-gateway:<tag>_amd64`
     - `pandaala/edgion-gateway:<tag>_arm64`
     - `pandaala/edgion-controller:<tag>_amd64`
     - `pandaala/edgion-controller:<tag>_arm64`

4. `create-release` (needs `build-and-push-images`)
   - Uses `softprops/action-gh-release` with `body_path: releases/<ref_name>.md`
   - Tags containing `-` are marked pre-release
   - Note: with `workflow_dispatch`, `github.ref_name` is the dispatched branch/tag — this job assumes a tag context, so it is only meaningful when the workflow is run against a `v*` tag ref

5. `merge-manifests` (needs `build-and-push-images`)
   - Extracts from `refs/tags/vX.Y.Z`:
     - Full version `X.Y.Z`
     - minor `X.Y`
     - major `X`
   - Merges the manifest, ultimately producing:
     - `:<full>`
     - `:<minor>`
     - `:<major>`
     - `:latest`

## Publishing a GitHub Release

> Because `build-image.yml` is currently `workflow_dispatch`-only, this is **no longer auto-triggered by a tag push**. You run the workflow manually (selecting the `v*` tag ref); the `create-release` job then creates the Release.

**Publishing steps:**

1. Write `releases/<tag>.md` (e.g. `releases/v0.2.0.md`) and commit it to the repo
2. Push the tag, then manually dispatch `build-image.yml` against that tag → after the workflow finishes pushing images, the `create-release` job creates a Release whose body is taken from that file

**File format:** plain Markdown, divided into sections such as `## Features` / `## Bug Fixes`, with a link at the end pointing to the README.

**Tags containing `-`** (e.g. `v0.2.0-beta`) are automatically marked as pre-release.

> Historical release notes live in the `releases/` directory; the filename is the tag name.

## Local reproduction commands

### Reproduce CI checks

```bash
cargo check --workspace --all-targets
cargo fmt --all
python3 cicd/checks/validate_agent_docs.py
cargo clippy --workspace --all-targets
python3 cicd/checks/check_ssa_force.py
cargo test --all
```

This block is the project quick/pre-commit sequence plus tests. Branch protection also
uses independent workflows for English-only content, Gateway hot-path tracing,
EdgionConfigData overlays, SSA force, and supply-chain checks. Inspect
`.github/workflows/` before claiming full CI parity.

### Reproduce the release binary builds

Shipped images use the `production` profile (LTO + `codegen-units = 1`, see the
root `Cargo.toml`), so reproducing the *shipped* artifact requires `--profile
production` — a plain `--release` build optimizes differently and lands in a
different directory. The commands below mirror the `build-binaries` job exactly
(`-p` package selectors + `production` profile); output lands in
`target/<triple>/production/`, not `target/<triple>/release/`.

```bash
# amd64
cargo build --locked --profile production \
  -p edgion-gateway -p edgion-controller -p edgion-cli \
  --bin edgion-gateway --bin edgion-controller --bin edgion-cli \
  --target x86_64-unknown-linux-gnu

# arm64
rustup target add aarch64-unknown-linux-gnu
cargo build --locked --profile production \
  -p edgion-gateway -p edgion-controller -p edgion-cli \
  --bin edgion-gateway --bin edgion-controller --bin edgion-cli \
  --target aarch64-unknown-linux-gnu
```

> The three main binaries are in `default-members`, so the bare
> `cargo build --bin edgion-gateway ...` form also resolves from the workspace
> root; the explicit `-p` selectors are kept here only to mirror CI verbatim.

### Reproduce the runtime image packaging

`BINARY_PATH` must point at the `production` output directory, matching the
`build-and-push-images` job (which downloads the `production` artifacts):

```bash
docker build \
  --file cicd/docker/Dockerfile.runtime \
  --build-arg BINARY=gateway \
  --build-arg BINARY_PATH=target/x86_64-unknown-linux-gnu/production \
  -t edgion-gateway:local .
```

## High-risk change points

- When changing `.github/workflows/*.yml`, first confirm whether the local action's inputs are still sufficient
- When changing tag naming rules, also check the version-extraction logic in `merge-manifests`
- When changing binary names, artifact paths, or target triples, also check artifact upload/download and `Dockerfile.runtime`
- When changing `cache-key-prefix` or cache paths, avoid different jobs polluting each other or fully losing reuse
- When changing CI commands, prioritize stating whether it is "aligning with common local commands" or "expanding CI coverage"

## Review checklist

- Are the workflow trigger conditions and branch / tag rules still correct
- Is the local action path still `./.github/actions/setup-rust`
- Does the `agent-docs` job still cover the entry layers `AGENTS.md` / `skills/` / `docs`
- Do the `fmt` / `clippy` jobs still install only what they actually need
- Does release still publish only the `gateway` / `controller` runtime images
- Are version tag, image tag, and manifest tag still consistent

## Related

- [00-local-build.md](00-local-build.md)
- [01-docker-build.md](01-docker-build.md)
- [../../02-features/01-binary-and-deployment/02-feature-flags.md](../../02-features/01-binary-and-deployment/02-feature-flags.md)
