---
id: ARCH-020
status: confirmed
kind: architecture-debt
priority: P2
primary_partition: PART-253
independent_verification: confirmed
---

# Normalize discovery geometry without constructing provisional runtime selectors

## Concrete cold call chain and cost

backends/discovery/coordinator.rs transact performs exact epoch-reservation
preflight through projection_epoch_requirement, build_sections and build_section.
build_section constructs LbBackend wrappers, all/ready arrays, direct index and
all four CompiledSelectors. After reservation, build_service_projection repeats
normalization. Changed available sections then reach materialize_section, which
assigns runtime identities, rebuilds arrays/index and compiles all four selectors
again. Provisional selector sets are neither published nor selected.

A valid changed section thus constructs complete selector sets three times,
including consistent-hash rings and round-robin maps. Semantic no-op transactions
also construct provisional sets before retaining the old projection. Adding a
slot-derived runtime field requires inspecting provisional and final assembly
although only final assembly is executable. This is observed construction and
split ownership, not a measured latency regression or incorrect epoch reservation.

## Bounded improvement and alternatives

Introduce private normalized geometry: checked port identity, numeric sockets,
readiness, originating proofs and typed invalid outcomes. Preflight counts and
fingerprints that same normalization. Final assembly alone creates identity-bearing
wrappers, direct runtime index and all four eager selectors. Initially retain two
pure normalization passes if retaining a transaction plan complicates reservation;
removing provisional executable state already addresses the concrete root.

Exact preflight is essential: it must share normalization with publication so
checked epoch demand cannot drift at exhaustion. Identity assignment, same-socket
transport-counter reuse and unavailable outcomes have separate purposes. Keep
all selector compilation cold; lazy request-time compilation or skipping preflight
would violate the existing boundary. Retaining provisional selectors is simpler
structurally but duplicates expensive runtime preparation without an execution role.

## Invariants, risks and future checks

Preserve sorted exact epoch demand/assertions, all-or-nothing reservation,
delete/no-op behavior at exhaustion, number/name alias sharing, heterogeneous
serving ports, merged origins, content fingerprints, transport-counter reuse only,
fresh wrapper/state identity, joint publication and fail-closed activation gaps.
The representation must not let preflight and final normalization diverge.

Existing coordinator exhaustion, overlapping updates, relabel/recreate, alias,
direct-index, section-isolation and retirement/completion tests must remain.
Count selector construction at the cold seam to verify none in preflight and one
per changed final section. No throughput claim follows from source inspection.

## Authority and review limits

The analyst fully read all ten discovery primaries with publication/selection
callers and current tests. Original endpoint-slice-ownership-transitions,
named-port-resolve-against-originating-slice, ep-slice-no-port-fail-closed and
service-runtime-snapshot-selection require exact proofs and eager final selectors;
none accepts provisional runtime construction. This differs from ARCH016's
backendRef immutable-lifecycle root. No executed checks; verification pending.
