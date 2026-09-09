# Route Manager Retained Listener Handle Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add live, cross-process coverage proving that a listener-bound proxy retaining a
per-listener route-manager handle stops serving removed routes after the manager
is removed from the global registry. Unit tests cover the manager lifecycle and
atomic empty-snapshot publication, but they do not keep a real Pingora proxy or
connection alive across configuration convergence.

## Requirements

- Cover HTTP and gRPC request paths that retain manager handles on
  `EdgionHttpProxy`, plus representative connection-oriented L4 coverage.
- Establish traffic through a non-empty route table, retain the relevant
  connection or listener generation, then remove the route and listener and
  wait for observable configuration convergence without fixed sleeps.
- Verify requests or frames using the retained serving object cannot reach the
  removed backend after convergence.
- Distinguish the removed manager from any newly created manager for the same
  `ListenerKey`; recreating an empty registry entry is not proof that the old
  retained handle was cleared.
- Cover both a full-rebuild stale sweep and a partial-update orphan sweep, or
  document why the integration driver can exercise only one while unit tests
  retain exact branch coverage.
- Preserve the single shared integration topology and existing timeout,
  process-lifecycle, and dynamic-scenario conventions.

## Scope

### In scope

- New integration fixtures or scenarios dedicated to retained listener handles.
- HTTP, gRPC, and one representative L4 protocol, with an explicit assessment
  of whether TLS and UDP need separate wire coverage.
- Assertions based on routing outcome and backend observations after config
  convergence.

### Out of scope

- Changing route-manager lifecycle semantics.
- Adding a second Controller or Gateway topology.
- Modifying existing integration files without separate authorization; the
  current integration suite is a protected validation baseline.

## Completion criteria

- The new coverage fails if registry removal leaves the removed manager's route
  snapshot non-empty and passes when removal publishes an empty snapshot.
- All affected protocol suites pass on Linux CI and macOS where supported.
- Any necessary registration change to an existing integration file is reviewed
  and authorized separately before implementation.
