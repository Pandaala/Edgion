# Discovery semantic digest preparation

Status: deferred for user decision. No runtime defect or replacement is selected.
Source baseline: db0f09890.

## Evidence

In edgion-gateway/src/backends/discovery/coordinator.rs,
semantic_digest_service clones the complete Service before removing resourceVersion,
managedFields, creationTimestamp, annotations, labels and status. It restores only
DNS_RESOLUTION_MODE, DNS_RESOLVER and DNS_RESOLVER_REF annotations.
semantic_digest_endpoint_slice clones the complete EndpointSlice and strips the
same metadata fields. Other fields retain their current serialization semantics.
semantic_digest serializes the sanitized resource to a Vec before SHA-256 hashing.
Its current error branch hashes empty bytes; this inspection does not establish
a reachable serialization failure for these resource types.

Both source-record staging functions compute digests on full and partial updates,
before transaction-level semantic no-op reuse. Large metadata that will be ignored
is nevertheless copied first; retained spec/endpoints are copied and serialized.
These are cold configuration costs, not per-request digest preparation.

The locked k8s-openapi 0.26.1 latest feature selects v1_34. Its generated Service
and EndpointSlice serializers explicitly order fields, emit apiVersion/kind and
omit absent optional fields. Replacing whole-resource cloning with a custom
borrowed projection must account for that contract and future dependency fields.
EndpointSlice Service-owner labels still drive reassociation outside the digest;
ignoring them in this digest does not make ownership changes irrelevant.

## Decision and alternatives

Retain the current implementation, or authorize measured investigation of:

- Borrowed sanitized serialization that avoids deep resource copying while
  preserving the existing serialized bytes and every included/excluded field.
  A manual projection introduces field-maintenance and dependency-upgrade duties.
- Streaming the existing serialized representation into the digest to avoid the
  serialized Vec. This is independent of resource cloning and must preserve
  all-or-empty error behavior rather than hash a partially written prefix.

No option is selected. Keeping the current implementation is valid if savings
do not justify additional machinery. Reuse an existing serialization/hashing
mechanism where its semantics fit; do not introduce a global digest cache, mutable
index, or unrelated cross-crate abstraction as part of this decision.

## Acceptance before implementation

Use a separately reviewed IMPL plan and exact file allow-list. Compare allocations,
bytes, peak retained storage and elapsed time for full resyncs, partial changes and
semantic no-ops, with small and large metadata/endpoints. Include retained output
and ordinary small resources; fewer copies alone do not establish a throughput win.

Keep the existing sanitizer/serializer as an independent test oracle. Require exact
digest equality for populated and absent fields, spec and endpoint variations, UID
and other retained metadata, ignored metadata/status, empty optional containers,
and the three retained DNS annotations. Preserve field order/omission and error
semantics, with a dependency-upgrade check against the generated resource types.
Do not accidentally broaden the ignored-field set.

Preserve source identity, content proofs, semantic no-op generation retention, DNS
annotation republishing, EndpointSlice relabel/recreate ownership, exact epoch
reservation, failure behavior, retirement and completion guards. Do not log resource
payloads or add request-path work, new observability or a second normalization policy.

Relevant originals: skills/01-architecture/02-gateway/07-backends.md,
skills/04-review/backends/regression-guards/discovery-normalized-geometry-ownership.md
and skills/04-review/backends/regression-guards/endpoint-slice-ownership-transitions.md.
This record is source inspection only: no runtime/test change, prototype, benchmark
or live transport experiment is included.
