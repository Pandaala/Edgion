---
name: discovery-normalized-geometry-ownership
description: Review Service discovery epoch preflight, normalized section proofs, final runtime materialization and selector construction ownership.
knowledge_type: regression-guards
---

# Pure discovery geometry precedes final runtime materialization

## Conclusion and scope

`ServiceDiscoveryCoordinator` normalizes checked Service port identities, numeric
sockets, readiness, deterministic merged origins and proofs into private geometry.
Typed invalid sections remain explicit. Exact epoch preflight and publication use
the same pure normalization. Neither normalization pass constructs executable
backend wrappers, all/ready runtime arrays, direct authorization indexes or selectors.

`materialize_section` alone creates fresh identity-bearing wrappers, runtime slot
arrays, the direct numeric index and all four eager policy selectors. Number/name
aliases share one materialized section. A changed Service materializes every unique
resolved section, including empty and individually unchanged sections; a semantic
no-op retains its old projection. Transport counters alone may survive through
same-proof/same-socket reuse. Eligibility state and wrapper identities remain fresh.

## Rationale and evidence

ARCH-020 identified three complete selector-set constructions for a changed
resolved section: provisional preflight, provisional projection construction, and
final materialization. Provisional selectors were never published or selected.
The private normalized owner removes these executable intermediates while
preserving exact checked epoch demand and fingerprint/proof byte order.

Authority: [backend architecture](../../../01-architecture/02-gateway/07-backends.md)
and [design rules](../../../00-design-rules.md), especially canonical ownership,
cold preparation, restrained observability and joint publication.

Sources: `edgion-gateway/src/backends/discovery/coordinator.rs` and
`edgion-gateway/src/lb/compiled_selector.rs`. The constructor's test-only thread-local
counter measures actual selector-set builds without inter-test shared counters.

## Alternatives and limits

Two pure normalization passes are retained to keep all-or-nothing reservation and
publication straightforward. This does not claim elimination of all cold allocations
or a measured latency/throughput improvement. Lazy request-time compilation and
skipping exact preflight would violate existing boundaries. Publication, fail-closed
activation gaps, failure cleanup and retirement/completion semantics remain unchanged.

## Verification and re-evaluation

The coordinator regression tests assert zero constructions in preflight, semantic
no-op, invalid-only publication and failed reservation; one per unique final resolved
section, including empty geometry; and exact terminal epoch success with a valid
number/name alias and invalid outcomes. Existing tests retain heterogeneous ports,
origin merge, direct-index authorization, recreate/relabel, counters, fresh state,
exhaustion and completion/retirement coverage.

Verified on 2026-10-12: backend tests passed 307 cases, including all 33 coordinator
cases; LB tests passed 122 cases. Focused policy and DirectEndpoint tests passed
8 and 3 cases respectively. Backend/LB execution used the same final test binary
freshly compiled by Cargo for the focused policy run, with serial test threads.
Workspace check and Clippy passed without warnings. Independent result review
approved the implementation and the coherent System-CA fixture correction;
production TLS admission and existing test assertions remained unchanged.
Full workspace tests, integration tests and performance measurements were not run.

At cleanup preflight, before adding this verification record, the 12-file reviewed
content revision
`ab3af8800e17936f999d9ae8a14f228d7c2a700492b5f0f1abd524d9625ba927`
was unchanged, and fresh agent-doc validation passed. The external WAF link failure
observed during implementation was no longer present.

Re-evaluate when normalized or runtime fields change, alias deduplication changes,
preflight and publication stop sharing normalization, fingerprint order changes,
selector compilation moves, or reservation/activation ownership changes.

## Resolution references

- ARCH-020: provisional executable discovery state replaced by private pure geometry
  and final-only eager runtime materialization.
