# Use Explicit Allocator Dependency Features Across Binaries

## Meta

- Status: pending
- Type: refactor
- Priority: P3
- Discovered in: Gateway quality campaign B0001

## Evidence

The Gateway, Controller, and CLI allocator features currently enable optional allocator dependencies through Cargo's implicit dependency-feature names instead of explicit `dep:` members. Changing only the Gateway would create an inconsistent cross-binary feature convention.

## Why deferred

This requires a workspace-wide decision and coordinated edits across three binary crates, while B0001 is anchored to the Gateway manifest and must remain behavior-preserving and local.

## Impact and open questions

- Determine whether hiding the implicit dependency features improves the intended public feature surface.
- Verify every binary's mutually exclusive allocator combinations and image/build invocations together.

## Candidate next step

Review the allocator feature contract across `edgion-gateway`, `edgion-controller`, and `edgion-cli`; if explicit dependency features are preferred, update all three manifests and their focused feature-matrix checks in one separate task.
