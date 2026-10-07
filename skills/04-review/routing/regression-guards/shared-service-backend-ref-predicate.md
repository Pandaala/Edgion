---
name: shared-service-backend-ref-predicate
description: Historical review evidence for Redundancy / L4 routing; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Shared service backend ref predicate

## Current reference contract

`edgion-resources::resources::refs::targets::is_standard_service` is the canonical
classifier. Omitted or empty group and omitted or exact `Service` kind identify a
standard Service. Literal `core`, case variants, ServiceImport, ServiceExternalName,
and other pseudo kinds are unsupported. Native Route fields additionally validate
source/target names and namespaces and require a Service port in `1..=65535` before
creating dependencies or reading authorization/material. The former Gateway raw predicate wrapper has been removed after all native
L4 consumers migrated to owning compiled targets. No native transport reclassifies
raw group/kind or accepts a literal `core` group.

Native HTTP/GRPC/L4 cold compilation obtains targets from the actual owner's effective
field loan, then keeps the Service key, checked section and policy target private.
HTTP/GRPC selection, SP, DirectEndpoint and final peers borrow that compiled
target. L4 retains original index slots and borrows a selected target through
the actual discovery gate; they do not reclassify raw fields or use context as reference authority.
Invalid/denied slots retain weights while skipping backend material lookups.
HTTP/GRPC also skip backend factories; L4 has no backend factory and preserves
its existing route-stage stream-plugin ordering. Effective loans are target projections and cannot mint Grant queries.

## Historical consolidation

The `path-norm-F006` change consolidated five duplicated L4 predicates into one
Gateway helper. That historical implementation accepted the `core` alias. The
later canonical reference contract supersedes that alias; the earlier report's
claim of unchanged behavior applies only to the original consolidation. It is
not evidence that the alias remains supported today.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `path-norm-F006` — 2026-05-26.
