# Business Mesh Gateway

- Created: 2026-09-25
- Status: Design and task breakdown recorded; implementation pending
- Profile: Investigation with linked implementation tasks
- Implementation and deployment validation: Pending

## Summary

Start with a business-owned Controller and Gateway deployment, a fixed namespace
scope, and existing routing, policy, and TLS resources. The initial route adds
no production capability, resource type, mesh mode, or plugin.

The intended Mesh Controller automatically manages business TLS/mTLS: CA
bootstrap or import, certificate issuance, renewal, Secrets, and TLS policy
references, without cert-manager. Existing Secret reload paths deliver credentials.
This automation is planned control-plane work beyond the configuration-only
baseline; it is not claimed as an existing capability.

## Documents

- [Istio Ambient + Edgion study](04-istio-edgion.md): current integration research
  priority, proposed topology, identity boundaries, and prototype plan.
- [Implementation task index](03-subtasks.md): eight ordered tasks with dependencies,
  scope, and acceptance criteria; start with task 01.
- [Overall specification](01-spec.md): baseline, certificate ownership, boundaries,
  and proposed validation.
- [Ambient investigation](02-ambient.md): what the additional layer does and what
  would require separate work.
- [Mesh roadmap](../00-roadmap.md): related tracks and earlier integration proposals.

## Working scope

The current deliverable is the design and implementation task breakdown. No
runtime implementation or deployment validation has been performed. Tasks 01-07
cover the initial business Gateway delivery; task 08 investigates Ambient.
Before implementation, re-read the
applicable repository knowledge and verify assumptions against current source.
Source inspection and historical tests are not fresh deployment verification.
