# Ambient feasibility and prototype decision

## Meta

- Created: 2026-09-25
- Status: in-progress
- Type: investigation
- Depends on: None for Istio integration research; review task 02 for shared assumptions
- Branch: Not assigned
- Output: [Initial Istio + Edgion assessment](../04-istio-edgion.md); prototype pending

## AI guide summary

Determine whether application-transparent transport is necessary and whether Edgion should integrate or implement the missing layer.

Current priority: reuse upstream Istio Ambient with Edgion as an explicitly
addressed, enrolled business gateway. Study this before implementing an independent
issuer or an Edgion-owned node transport. Existing tasks 01-07 remain an alternative
delivery track; they are not prerequisites for this experiment.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Compare the verified business Gateway baseline with requirements for unchanged plaintext applications and workload identities.
- Study existing Ambient integration versus an Edgion-owned node transport/capture design, including HBONE and original destination handling.
- Define identity authorization, namespace/node ownership, bypass enforcement, failure behavior, and credential delivery.
- Return a bounded prototype proposal: two unchanged applications across nodes, authenticated secure transport, then optional Edgion L7 governance.

## Acceptance

- [ ] A recommendation identifies reusable Edgion capabilities, external dependencies, missing contracts, and operational costs.
- [ ] The proposal distinguishes ordinary ingress integration from waypoint compatibility.
- [ ] A separate implementation task is created only after the approach is selected; this investigation is not a prerequisite for tasks 01-07.

## Out of scope

Implementing CNI/node proxies or claiming compatibility based only on an architecture diagram.
