---
name: test-service-schema-regeneration
description: Use when reviewing the explicit test-package generator, its source schema, or checked-in gRPC stubs.
knowledge_type: regression-guards
---

# TestService schema regeneration

## Conclusion and scope

The explicit `edgion-tests/src/build_test_proto.rs` helper consumes the annotated
`edgion-tests/proto/grpc_json_transcoding/test.proto` and publishes the checked-in
`edgion-tests/src/proto_gen/test.rs`. The schema is shared with the transcoder
fixture descriptor. Missing schema is an error even when an old artifact exists.
Generation and repository formatting finish in a temporary directory before a
same-filesystem rename publishes the artifact. Failure before publication keeps
the previous file. The helper resolves paths from its compile-time crate directory.

The full package includes CodecTest/OverlapTest stubs and WKT types. These are
fixture definitions, not registered network services. Google API annotations
produce auxiliary Rust output that this package does not reference; it remains
temporary. Actual client/server compilation validates that dependency boundary.

## Rationale and evidence

ARCH-260 identified absent filename probes and a success fallback that kept stale
Rust output. That hid the missing regeneration owner. Reusing the live annotated
schema removes the separate maintenance decision without duplicating messages or
splitting descriptor fixtures. The wrapper's original four-parent directory
calculation was already correct and remains unchanged.

The [maintenance instructions](../../../../edgion-tests/proto/grpc_json_transcoding/README.md)
own the commands and prerequisites. Existing client/server includes, TestService
fields/tags, RPC paths and unary/server-streaming cardinality remain unchanged.
Ordinary preparation consumes checked-in stubs; Common retains its independent
build-time generation. Descriptor bytes and embedded fixtures are not rewritten
by Rust stub generation.

## Alternatives and limits

Generating the complete package accepts unused fixture-code growth in exchange
for avoiding custom filtering or a descriptor migration. The optional command
requires protoc and rustfmt. A process killed before publication can leave a
temporary directory, but does not partially overwrite the checked-in artifact.
This is not a repository-wide protoc-free build guarantee or a runtime change.

## Verification and re-evaluation

Focused helper regressions cover missing and malformed schemas preserving the
old artifact, with temporary staging cleaned after a generation error. Acceptance
also requires compilation of both existing include consumers. Fresh local checks
passed real root/alternate-directory generation, repeat-output comparison, missing
protoc and failing rustfmt artifact preservation, repository formatting, and exact
original TestService structs/service code comparison modulo comments/format.
Task records hold the executed commands and remaining consumer-build results. Re-evaluate when changing schema imports, generated
package references, generator versions, service registration or publication.

## Resolution references

- ARCH-260: declared schema ownership and truthful explicit regeneration failure.
