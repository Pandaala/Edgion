---
id: ARCH-260
status: candidate
kind: architecture-debt
priority: P3
partition: PART-436
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# TestService regeneration has no declared input-schema owner

## Current ownership and evidence

The optional `build_test_proto` developer binary is declared in
`edgion-tests/Cargo.toml:45` and says it regenerates
`edgion-tests/src/proto_gen/test.rs`. Its full implementation checks only
`edgion-tests/proto/test_service.proto` (twice) and
`edgion-tests/src/proto/test_service.proto` at lines 14-18. Neither source is
present in the current repository. At lines 22-28, an existing generated file
therefore converts missing input into a successful return without generation.
The only wrapper, `integration/scripts/proto/generate_test_proto.sh`, runs this
binary and then prints generation complete; it supplies no other schema.

The checked-in generated file declares the four TestService messages and the
SayHello/StreamNumbers client/server contracts. Both `server/test_server.rs:3536`
and `client/suites/grpc_route/mod.rs:5` include it; actual unary and streaming
suite bodies use those generated messages and methods. Ordinary builds use
this checked-in TestService artifact without running its regeneration helper.
This is not a repository-wide protoc-free build guarantee: the existing
edgion-common build script separately compiles conf_sync and ext_proc schemas.

There is a live matching TestService schema at
`edgion-tests/proto/grpc_json_transcoding/test.proto:8`, with the same package,
method paths, message fields and tags, plus HTTP annotations. Its original
README explicitly says these fields match the test-server service. It also
owns additional codec and overlap fixtures and its descriptor regeneration
path. The developer stub generator neither reads nor names this schema.
This finding is not a claim that no TestService definition exists anywhere.

## Concrete change scenario and structural cost

Changing a TestService field or updating tonic-generated stubs lacks one
explicit schema-to-stub path. The documented stub command succeeds while
retaining old output; a developer must discover the separate descriptor schema
and decide whether to edit generated Rust, supply an absent input, or redesign
regeneration. The descriptor and client/server artifact can then evolve from
separate manual decisions. This is a maintenance boundary problem, not an
observed wire mismatch, runtime failure or incident.

## Bounded improvement and alternative

Declare a real repository-owned source for TestService stub regeneration and
make the developer command consume it. Prefer the existing matching schema or
a reviewed shared schema arrangement; do not add another independent copy of
the same four messages solely to satisfy the stale filename probes. Review the
existing schema's annotations, imported well-known types, extra fixture services
and generated Rust dependencies before selecting the exact build configuration.
Fail the explicit regeneration command when its declared input is unavailable,
and only announce completion after generation succeeds.

Keep the checked-in TestService Rust artifact and leave ordinary integration
preparation independent of TestService stub regeneration. Preserve existing
Common build-time schema generation. This does not request automatic TestService
generation, a new registry or edits to existing integration test assertions. If regeneration
is intentionally unsupported, remove the misleading optional command/wrapper
and document the actual artifact-maintenance owner instead; that is a narrower
alternative, with the explicit cost of manual generated-code maintenance.

## Invariants, risk and future verification

Preserve package `test`, both RPC paths, field tags, unary/server-streaming
cardinality, test-server and client includes, and existing descriptor/annotation
fixtures. Preserve all CodecTest/OverlapTest and WKT test semantics. Verify the
chosen generator on the declared input, missing-input failure and current
client/server compilation. Check descriptor/stub TestService parity without
introducing network tests or production visibility for mocks. Keep generation
an explicit developer operation, outside existing runtime runners. No execution
was performed during this review and no generated-output equivalence is claimed.

## Original knowledge and deduplication

Read original root rules, graded design rules, testing/review indices and routing,
full integration architecture and suite map. The guide's shared checked-in
framework and build-before-runtime requirements are preserved. Searches by
`build_test_proto`, `test_service.proto` and generation symbols found the optional
wrapper, manifest, consumers and matching transcoder source; no existing pending
fix or accepted decision for this generator's absent probes was found. The
current fallback is explicit code behavior, not by itself proof of an accepted
schema-maintenance contract. Independent static verification confirms this
bounded maintenance issue; generation and compilation checks remain future
implementation validation.
