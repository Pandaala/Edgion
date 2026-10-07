---
name: grpc-descriptor-configmap-ownership
description: Use when reviewing GrpcJsonTranscoder ConfigMap acquisition, dependency and status ownership.
knowledge_type: regression-guards
---

# GrpcJsonTranscoder descriptor references retain actual ownership

## Conclusion and scope

EdgionPlugins GrpcJsonTranscoder configMapRef descriptors select only the fixed
core ConfigMap in their actual owner's namespace. An omitted owner namespace
defaults to default; an explicit malformed owner identity is rejected. The exact
binaryData key is part of the reference-only topology. Inline descriptor bytes
create no Kubernetes dependency and never enter that topology.

Only enabled supported request entries acquire material. Preparse and parse clear
previous descriptor material and errors in all four typed stages before binding.
Readonly status uses the same fixed target and permission pipeline without adding
dependencies. Identity errors report InvalidReference. Invalid key shape, missing
binaryData key, byte-cap and descriptor compilation errors retain the intrinsic
Accepted failure contract; a missing ConfigMap additionally reports ResolvedRefs.

## Rationale and evidence

The actual owning adapter is in Resources refs/plugin_fields.rs and owned.rs.
GrpcDescriptorReference has a private constructor and exposes its reference/key
only after identity and selector validation. Name, key, branch, enable and owner
drift invalidate a prepared loan; Controller material writeback does not.
Controller handlers/plugins/grpc_descriptor.rs consumes this loan through
credential/pipeline.rs. It never falls back from binaryData to data and cannot
choose another namespace, group or kind. Shared key-shape validation is the
unchanged crate-private grpc_transcoding::valid_descriptor_key predicate.

The applicable authority is the root AGENTS.md cross-cutting unification, serial
workqueue and schema/transport rules. This fixed local field has no authored
cross-namespace capability and requires no ReferenceGrant query for acquisition.

## Alternatives and limits

This is not a CA reference: converting ConfigMap contents to a Secret wrapper or
reading ca.crt would grant the wrong material semantics. Protobuf services[] are
not Kubernetes Service references. No new Gateway API field or CRD change follows.
HTTPRoute rule ExtensionRef remains the only supported runtime attachment.
The generic dependency-status scan and the complete ExtensionRef traversal audit
remain unfinished in tasks/pending/resource-reference-contract/.

## Verification and re-evaluation

Scoped Astra implementation review passed. Fresh owning Resources tests passed
65 tests; Resources doctests passed 16 with 10 existing ignored examples. Fresh
Controller plugin handler tests passed 107 after sharing the key predicate.
Coverage includes malformed source/name/key with no dependency, default namespace,
all-stage clearing, exact binaryData selection, recovery and replacement/delete,
readonly status and actual Accepted/ResolvedRefs conditions.
Re-evaluate if descriptors gain namespace/group/kind fields, another storage branch,
another plugin stage or runtime attachment, or different missing-key status rules.
These results do not establish whole-task or live conformance completion.

## Resolution references

- core-api-group-alias-contract: in-progress unified reference migration, approved
  current-HEAD GrpcJsonTranscoder inventory addition.
