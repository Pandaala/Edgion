# Complete RegionRoute Effective-Table Integration Validation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | testing |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Complete cross-process validation for the RegionRoute effective-table contract without weakening the current Controller and Gateway validation boundaries.

## Requirements

- Migrate the existing overlapping `region-route-myregion` fixture to the valid-table contract before using it as a positive scenario.
- Verify base and overlay hot reload, fallback, recovery, and per-consumer modulo validation through the shared Controller and Gateway topology.
- Assert Controller `Accepted` and `ResolvedRefs` status outcomes together with the Gateway effective-table result.
- Cover the Controller management effective and failover endpoints, including invalid-edit `422` responses and proof that rejected edits are not persisted.
- Preserve the canonical RegionRoute table/validation implementation and existing integration topology; do not create a second Gateway or Controller process.
- Follow the integration freeze workflow. Existing integration files require separate explicit scope and review before modification.

## Acceptance criteria

- Valid base/overlay updates and recovery publish the expected effective table across process boundaries.
- Invalid interval, reference, and consumer-modulo inputs produce the documented `Accepted`/`ResolvedRefs` status and fail-to-base or exclusion behavior; rejected management failover edits leave the persisted resource unchanged.
- Targeted integration checks and all repository-required guards pass.
