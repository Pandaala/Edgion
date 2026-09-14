---
name: ext-proc-conformance-evidence
description: Use when reviewing ExtProc compatibility claims and test-tier placement; separates wire schema, implemented phases, deterministic fast evidence, and slow external interoperability.
status: fixed
finding_id: ext-proc-conformance-coverage
closed: 2026-08-26
knowledge_type: regression-guards
---

# ExtProc Conformance Evidence — FIXED BASELINE

## Conclusion

An Envoy-compatible protobuf package and field numbers prove structural wire
compatibility only. An ExtProc behavior claim also needs a wired HTTP phase,
matching mutation or terminal semantics, and a passing test linked from the
compatibility matrix.

Default tests use Edgion's deterministic fake processor. Wall-clock idle,
stress, repeated transport combinations, certificate rotation, and an
independently generated processor belong behind `--full-test`.

ExtProc client state-machine unit tests are network-free. They drive
`ExtProcStream` through bounded request/response channels and reserve real
gRPC transport, HTTP/2 framing, TLS, and peer-observed half-close evidence for
the integration tier.

## Core rationale

1. Envoy's own ExtProc suite primarily uses repository fake/mock processors;
   it is a strong source of protocol cases but not third-party interoperability
   evidence for Edgion.
2. Edgion intentionally rejects response buffering, partial buffering,
   `FULL_DUPLEX_STREAMED`, and post-commit streamed local replies. Missing tests
   for those modes are not product defects.
3. Trailer message types retained in the protobuf do not imply that the HTTP
   proxy produces or consumes trailer phases.
4. Short deterministic failure and TLS/mTLS cases stay in the default suite.
   Real waits and stress are opt-in so the default feedback loop remains fast.
5. Internal stream transitions and exact error mapping do not require a socket.
   The test-only `InboundState::Test` receiver exercises the same
   `exchange`/`finish_success` state machine while the integration processors
   prove that tonic carries those messages and terminal states over the wire.

## Unit isolation boundary

The channel-backed `client.rs::tests::stream_state` cases retain the former
live-server assertions for:

- request headers followed by multiple body chunks;
- request and response phases on one ordered logical stream;
- response body followed by response trailers;
- final-body and final-trailer outbound EOF;
- extra terminal verdict, terminal status, and terminal-drain timeout errors;
- a second exchange after outbound half-close.

The unit selection contains no listener, server builder, or network exchange.
The default and full ExtProc integration suites remain responsible for the
actual gRPC transport, phase/EOS observations, response trailers, normal
half-close, timeout behavior, and TLS/mTLS connections.

## Fix suggestions not accepted

- Claim complete Envoy compatibility from protobuf field-number parity.
- Copy every Envoy body mode into Edgion despite the streaming response
  invariant.
- Put wall-clock waits or external downloads in the default integration sweep.
- Treat an Edgion fake processor as independent interoperability evidence.

## Re-evaluation triggers

- Edgion wires request trailer phases.
- The version-pinned external processor fixture changes generator version or
  ownership model.
- The response-body invariant changes through an approved design.

The former response-trailer and external-fixture triggers were re-evaluated on
2026-08-30. Response trailers now have a wired Gateway lifecycle, and the
owned full-test fixture uses official Go v1.37 generated types. The
compatibility matrix and the focused 32-case full suite provide the behavioral
evidence; neither fact broadens support to request trailers or unsupported body
modes.

## Reference cases

- Compatibility matrix:
  `edgion-tests/integration/conf/EdgionPlugins/ExtProc/COMPATIBILITY.md`
- Fast/full integration suite:
  `edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs`
- External interoperability and stress fixture:
  `edgion-tests/integration/fixtures/ext_proc_external/README.md`

## Resolution references

- `ext-proc-conformance-coverage` — 2026-08-26.
- `isolate-ext-proc-live-stream-unit-tests` — 2026-09-14. The 25 focused
  ExtProc client tests passed after the live tonic servers were replaced with
  deterministic bounded channels; this focused result is not a fresh run of
  the integration suite.
