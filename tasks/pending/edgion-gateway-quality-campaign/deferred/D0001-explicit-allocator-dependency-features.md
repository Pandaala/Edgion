# D0001 — Explicit Allocator Dependency Features

## Meta

- Status: resolved
- Severity: low
- Discovered in: B0001, `edgion-gateway/Cargo.toml`
- Category: maintainability

## Evidence

All three product binary manifests expose allocator optional dependencies through Cargo's implicit feature names.

## Why deferred

A Gateway-only edit would create cross-binary inconsistency; the issue needs a coordinated workspace-wide task.

## Resolution

- The Gateway, Controller, and CLI manifests now use explicit `dep:` entries for both optional allocator dependencies.
- `skills/04-review/supply-chain/regression-guards/allocator-feature-surface.md`
