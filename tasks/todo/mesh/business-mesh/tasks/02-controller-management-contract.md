# Controller management contract

## Meta

- Created: 2026-09-25
- Status: pending
- Type: feature
- Depends on: 01
- Branch: Not assigned
- Output: Pending

## AI guide summary

Define the smallest configuration and ownership contract for automatic TLS/mTLS before writing the reconciler.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Choose enrollment and configuration using existing Controller configuration where sufficient; justify any schema extension rather than assuming a Mesh CRD.
- Define fixed namespace scope, explicitly selected Services, TLS versus mTLS, CA storage reference, certificate identity/SAN rules, and resource names.
- Define who owns generated Secrets/BTP/frontend resources and how existing user-owned resources and competing policies are handled.
- Specify backend certificate installation and reload for the supported first target; begin with a TLS-capable application fixture.
- Inspect conf_sync CA, certificate generation, lifecycle, and relevant review knowledge for reusable mechanisms. Keep business and conf_sync trust separate.
- Specify signing/scheduling/API IO outside resource processing handlers, existing watch/requeue publication, replica coordination, and startup/shutdown behavior.

## Acceptance

- [ ] A concrete configuration example and resource ownership table cover two Services and one shared Gateway pool.
- [ ] The design explains both sides of each TLS/mTLS hop, forbidden identities, conflicts, and removal behavior.
- [ ] The implementation plan identifies reusable code, minimal changes, tests, and unresolved prerequisites; the chosen configuration contract is recorded before implementation.

## Out of scope

Implementing Ambient, creating a separate binary by default, or generic certificate management for unrelated workloads.
