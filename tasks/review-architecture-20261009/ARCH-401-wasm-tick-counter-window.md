---
id: ARCH-401
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-517
analyst: /root/analyze_conversion
baseline: ffe2e6a21dcf399b001d30cd1cd8ff5ae780eee347dc41ceb67fe052dccc6a29
---

# Require fresh Wasm tick progress in the observation window

`test_tick_shared_data` and `test_tick_callout` in `edgion-tests/src/client/suites/edgion_plugins/wasm/wasm.rs:427–520` sleep and accept a returned counter whenever it is at least one. They describe live tick execution and a completed tick-phase callout, but these counters belong to the configured Wasm plugin lifetime. The reference guest cumulatively increments `ticks`; its root callback increments `calloutticks` only for an HTTP 200. Request probes merely read those shared values. Current plugin-owned runtime isolation retains the same shared-data store across ordinary requests without resetting it between suite invocations.

After one successful background callback, an unavailable auth dependency can stop subsequent successful callouts while the old `calloutticks` value remains positive. A later invocation of the registered tick-callout case still reports current success. Likewise, an unchanged positive tick counter does not prove another tick during this case. These are conditional observation witnesses, not a reported outage or a claim that current production scheduling is broken. Earlier request-phase callout cases and real fresh-runtime tick unit tests remain useful, but they do not turn an old cumulative value into evidence of progress in this case's window.

Read a successful baseline with the existing guest marker/header, then poll the same counter under a bounded deadline until a later successful probe shows a positive checked delta. Require numeric headers and fail on malformed observations, counter reset or a non-growing value at the deadline. Preserve both independent tests, their actual request-stage runtime and fixture host, valid auth origin and grants, HTTP outcomes and JSON shape assertions. A small private counter probe can reuse the present client and observation headers; no guest reset endpoint, fresh Gateway requirement or production instrumentation is needed.

Longer sleeps or a larger absolute minimum are inadequate alternatives: old state can satisfy either. Resetting counters hides lifetime behavior and mutates shared runtime state solely for testing. The original body-counter delta guard is explicitly body-scoped; its verified cumulative-state rationale supports this proposal without claiming it already required or implemented tick deltas. Current instance-isolation and owner-shutdown decisions remain unchanged.

Future verification should fail with a positive unchanged counter, accept later growth from a nonzero baseline, reject missing/reset counters and preserve repeated local/Kubernetes execution on the shared topology. Retain actual tick scheduling, callback failure handling, cancellation, epoch deadlines, canonical bounded egress and secret-safe observability; do not add metrics or request-path copies for mocks. Current historical successful runs do not verify the false-pass branch. All primary code and decisive guest/runtime/fixture/unit/original seams were personally read; no tests or external actions were executed.
