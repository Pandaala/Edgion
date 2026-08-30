# Weighted Invalid BackendRefs Retain Their Traffic Share

## Decision

Ordinary HTTPRoute backend selection includes both compiled Service targets and invalid backendRef slots in its weighted selector. A selected invalid slot is forced to `EdgionService::Unsupported` from its compiled target classification before peer selection, preserving the failure share declared by its `weight` without reinterpreting only the raw `kind`. Silently removing invalid slots, redistributing their weight to healthy siblings, or routing an invalid-group Service-family name is not accepted.

Valid `EdgionBackend` targets remain excluded from the ordinary selector because the AI dispatch plan owns their ordered, weighted selection. An invalid AI-shaped reference, such as one with a forbidden port, is an invalid slot and retains its failure share like any other invalid backendRef.

## ServiceImport classification

Core-group `ServiceImport` is an Edgion Service-family kind because `EdgionService::from_kind` owns its peer-selection result. It therefore compiles as a Service target and reaches the explicit `BackendServiceImportNotImplemented` status. Treating it as an unsupported ref before weighted selection loses that diagnostic and changes mixed-backend traffic distribution.

Service-family names under a non-core group remain unsupported.

## Regression coverage

The canonical resource classification test includes core-group `ServiceImport`. The compiled-route selector test combines weighted Service, ExternalName, ServiceImport, valid AI, invalid AI, and a non-core-group `Service`. It requires exact WRR counts that retain both invalid shares while selecting the valid AI slot zero times. A peer-context test additionally requires the selected non-core `Service` to remain unsupported rather than being reinterpreted by its raw kind.

This restores the existing Gateway API weighted-invalid-ref behavior and changes no schema or operator-facing configuration, so no user documentation update is required.
