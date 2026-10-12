# Establish a production AI cost and capacity baseline

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P1 |
| Tier | Future verification; previously completed scoped checks remain complete |
| Dependencies | Current supported native APIs and existing test harness; no optional roadmap feature is required |

## AI guide summary

Finish the explicitly bounded remaining verification scope below without reopening
completed local implementation or inferring broader acceptance from historical passes.

## Completed foundation and evidence limits

A local ordinary-release/integration-mode Chat H1 run on 2026-10-12 passed
80 timed cohorts with 244,983 validated requests, 12 long streams, four real
stopped-socket plateau/resume/cancel probes, 23 SingleProvider cases, ordinary
controls and strict residue. Workspace check/Clippy and independent final review
passed for that scoped implementation. Preserve and reuse the existing harness.

SSE observation added paired median Gateway CPU costs of approximately 55 and
66 microseconds/request at concurrency one/eight. The single-process HTTPX driver
used about 83-84% of one core at SSE concurrency eight, while the Gateway used
18-25%; increased concurrency did not improve throughput. Host contention and
swapping were present. RSS did not grow proportionally with tested stream length,
but compressible payloads and RSS sampling did not establish a live-heap bound.
This is exploratory evidence, not production capacity, a release SLA or proof of
an inefficient Gateway function. Refresh the baseline after source changes.

## Requirements and scope

- Remove the load-driver limitation with bounded, validated multiple-process or
  equivalent load generation and verify that the fixture is not the bottleneck.
- Select the actual shipped build/profile, allocator and production runtime mode;
  record compiler, binary hashes, versions, hardware, host load and topology.
- Keep direct fixture, ordinary proxy, native observation off/provider workloads
  equivalent. Extend coverage to the declared native Chat/Responses/Messages
  matrix and H1/H2 transports, with supported encodings and connection reuse.
- Measure per-request CPU, latency distributions, throughput, allocation/live-heap
  behavior and concurrency limits. Separate transport first-byte timing from model
  TTFT; do not infer capacity from a driver-limited closed-loop RPS result.
- Use representative long prompts, tool schemas, high event rates, long SSE and
  slow clients. Enforce digest/byte checks and actual stopped-socket plateau,
  resume, cancellation and once-log assertions; include reload under traffic.
- Profile functions only after reproducible paired measurements identify a useful
  cost target. Separate parsing, canonical logging/metrics and other path work.
- Measure conversion, quota and guards only when actually enabled. For selected
  unary conversion, retain the existing ownership contract and separately measure
  allocation overlap, concurrent admission, complexity limits and cancellation.
- Agree thresholds from evidence before making them release gates. Measure disabled
  mechanisms for absence of work; do not accept an integrity PASS as good performance.

### Out of scope

Universal latency promises, live-provider stress traffic, automatic production
refactors from noisy results or expanding response aggregation permissions.

## Acceptance criteria

- [ ] Baselines reproduce on current code with an adequate load driver and explicit
  sampling/host/build limitations; saturation claims include resource evidence.
- [ ] Resident memory and live allocations are distinguished under representative
  workloads; total stream length does not introduce unbounded response retention.
- [ ] Required native/transport cells, actual socket lifecycle, ordinary controls,
  hot reload and strict residue have traceable results.
- [ ] Disabled mechanisms avoid their respective runtime work; enabled costs have
  reproducible attribution before any optimization proposal.
- [ ] Agreed scoped thresholds are met or deficiencies are reported explicitly.

## Source entry points

- [edgion-tests/integration/scripts/integration/benchmark_ai_native.py](../../../edgion-tests/integration/scripts/integration/benchmark_ai_native.py)
- [edgion-tests/src/server/ai_performance.rs](../../../edgion-tests/src/server/ai_performance.rs)
- [edgion-tests/integration/scripts/integration/run_integration.sh](../../../edgion-tests/integration/scripts/integration/run_integration.sh)
- [edgion-gateway/src/ai/runtime/dispatch.rs](../../../edgion-gateway/src/ai/runtime/dispatch.rs)

## Independent implementation boundaries

This is future work, not authorization to implement or call a paid provider now.
Revalidate the current source and applicable review knowledge when starting; code
pointers below are investigation entry points, not verified completion claims.

- Edgion is a proxy-layer gateway. Preserve client-owned model choice and native
  payloads. Keep conversion and usage observation explicit; observation defaults off.
- Reuse canonical credentials, auth/key extraction, routing, snapshots, retries,
  deadlines, Redis limits, attempt accounting and final access logging. No agent
  runtime, hosted tool execution, MCP/A2A gateway or new parallel infrastructure.
- Main upstream responses stream. No response BodyStorage, disk spool, unbounded
  aggregation or second SSE window. Semantic windows use bounded RawWindow. The
  existing selected unary Chat-to-Anthropic converter has its own strict memory,
  complexity, admission, deadline and precommit-error contract; do not extend that
  exception to another converter without a separate reviewed design.
- Compile policy before request processing. Preserve cancellation cleanup,
  all-attempt accounting and Controller serial workqueue discipline.
- Gateway traffic diagnostics belong to canonical access logs and bounded metrics;
  no request-path tracing, payload/credential logging or identity/model metric labels.
- Any operator-facing schema change needs CRD coverage; internal resolved fields
  retain schema hiding, conf_sync serialization and secret redaction.

## Knowledge and verification

Read [project rules](../../../AGENTS.md), then the [knowledge index](../../../skills/SKILL.md)
and [design rules](../../../skills/00-design-rules.md). Load the relevant original
[AI/backend architecture](../../../skills/01-architecture/05-resources/22-edgion-backend.md),
[review index](../../../skills/04-review/SKILL.md) and
[testing index](../../../skills/05-testing/SKILL.md) on demand. Those project
sources remain authoritative; this task is self-contained as a work specification.

Use a reviewed plan for complex implementation. Add meaningful targeted unit,
integration and hot-reload tests; preserve ordinary HTTP/native/SSE controls and
strict residue. Put unit bodies under the owning crate's tests/unit tree. Run the
applicable project checks and update current capability/user documentation.
Run cargo test --all only when explicitly requested. Paid calls require a concrete
endpoint/model/credential-reference/call-count/budget scope authorized for that run.
