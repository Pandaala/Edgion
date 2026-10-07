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
creating dependencies or reading authorization/material. The Gateway's
`routes/common.rs::is_core_service_backend_ref` delegates to the shared predicate;
its name does not imply acceptance of a literal `core` group.

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
