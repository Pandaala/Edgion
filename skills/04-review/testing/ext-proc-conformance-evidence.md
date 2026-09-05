---
name: ext-proc-conformance-evidence
description: Use when reviewing ExtProc compatibility claims and test-tier placement; separates wire schema, implemented phases, deterministic fast evidence, and slow external interoperability.
status: fixed
finding_id: ext-proc-conformance-coverage
closed: 2026-08-26
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
