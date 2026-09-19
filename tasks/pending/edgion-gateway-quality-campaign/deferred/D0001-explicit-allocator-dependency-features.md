# D0001 — Explicit Allocator Dependency Features

## Meta

- Status: ready-for-separate-task
- Severity: low
- Discovered in: B0001, `edgion-gateway/Cargo.toml`
- Category: maintainability

## Evidence

All three product binary manifests expose allocator optional dependencies through Cargo's implicit feature names.

## Why deferred

A Gateway-only edit would create cross-binary inconsistency; the issue needs a coordinated workspace-wide task.

## Canonical task

- `tasks/todo/use-explicit-allocator-dependency-features.md`
