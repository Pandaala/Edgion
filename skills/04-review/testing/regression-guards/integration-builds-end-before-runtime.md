---
name: integration-builds-end-before-runtime
description: Use when reviewing Cargo invocations, integration preparation, or --no-prepare behavior.
knowledge_type: regression-guards
---

# Integration builds end before runtime

`utils/prepare.sh` is the single build boundary for the main and aggregate integration runners. It
must compile every binary and ignored-test executable used later, including validators. Runtime
scripts execute stable artifact paths and fail fast when an artifact is absent; they never invoke
Cargo after services, containers, or scenarios have started.

When Cargo emits a hashed integration-test executable, resolve it during preparation and copy it to
a stable path under the active target directory. Aggregate children receive their no-build option
explicitly. A standalone script may invoke the canonical prepare step for convenience, but it must
finish before any runtime fixture starts and its no-build mode must consume the same prepared
artifact.

The main runner's `run_test` helper expands a command string with `$cmd`; quote characters embedded
inside that string are data and are not parsed again by Bash. Stable artifact invocations must therefore
follow the runner's existing no-space path contract and must not wrap the executable in literal quote
characters. Keep the executable check (`-x`) before dispatch, and cover the exact command shape with a
structural regression test until the runner is migrated as a whole to an argv-safe API.

## Resolution references

- `ITEST-008` — 2026-08-26.
