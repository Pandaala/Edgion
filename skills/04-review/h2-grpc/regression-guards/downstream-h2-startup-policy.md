---
name: downstream-h2-startup-policy
description: Review bounded process-level downstream H2 tuning, default protection, startup validation, and protocol wiring.
knowledge_type: regression-guards
---

# Bounded downstream HTTP/2 startup policy

## Conclusion and scope

P090-010 was a missing operator configuration surface, not evidence of unsafe
defaults or a measured performance defect. Gateway process `server` configuration
now exposes optional `downstream_h2_stream_window_size`,
`downstream_h2_connection_window_size`, and `downstream_h2_max_concurrent_streams`.
Explicit CLI fields override YAML individually. Omission preserves Pingora defaults:
65535-byte receive windows, 100 concurrent streams, and a 64 KiB decoded header cap.

The policy applies uniformly to normal HTTP proxy services supporting h2c or TLS
H2. It does not enable a disabled protocol, change H1, tune upstream connections,
or promise hot updates to open connections. Changes require process restart.

## Rationale and evidence

- `edgion-gateway/src/cli/config.rs`: optional fields, explicit CLI merge, and
  validation during local config load before services start.
- `edgion-gateway/src/runtime/server/downstream_h2.rs`: canonical validation and
  construction from Pingora's `default_h2_options()`. Raw `H2Options::default()`
  would omit the explicit header/concurrency protection.
- `edgion-gateway/src/cli/pingora.rs`: prepare the policy before Pingora bootstrap,
  also defending callers that bypass local config loading.
- `edgion-gateway/src/runtime/server/base.rs` and `listener_builder.rs`: carry
  one validated policy to every normal proxy and install it through
  `Service::app_logic_mut()` before adding TCP/TLS endpoints. Both protocols
  consume `HttpProxy.h2_options` through the common Pingora H2 handshake.

Windows are 65535..=16777216 bytes, streams are 1..=1024, and effective stream
window times effective concurrent streams must not exceed 67108864 bytes.
Multiplication uses `u64` after validating `u32` fields. These are Edgion's
conservative operator limits, not limits mandated by HTTP/2 or Pingora.

## Alternatives and limits

No fork setter or standard Gateway API CRD modification is required. Per-Gateway
and per-listener policies are outside the accepted process-level scope.
The combined credit budget is not a hard bound on process memory, all connections,
filter buffers, or total uploaded bytes. Larger windows are not an unconditional
throughput improvement and do not directly increase client download receive credit.

The user explicitly requested simple tests instead of the original task's full
RTT/load and gRPC/TLS lifecycle acceptance matrix. The focused protocol test
checks the production policy-to-proxy seam and actual H2 SETTINGS/WINDOW_UPDATE
frames over in-memory I/O. It does not perform a real TLS handshake, start the
Gateway/Controller topology, benchmark throughput/memory, or prove every stream
cancellation/backpressure case. No performance claim is made.

## Verification and re-evaluation

Focused tests cover config parsing/CLI precedence, invalid local-config rejection,
individual and combined bounds, unchanged omitted settings, configured peer
settings/windows, and retained header protection. The protocol observer waits for
the server's first idle poll before sending its completion PING, so the scheduled
initial connection WINDOW_UPDATE cannot be overtaken by the completion marker.
This ordering check relies on the fixed 64 KiB in-memory transport holding the
small initial frame exchange without backpressure.

Fresh focused results in this run:

- `cargo test -p edgion-gateway --lib runtime::server::downstream_h2`: 4 passed.
- `cargo test -p edgion-gateway --test downstream_h2_config`: 2 passed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed.
- Repository-required agent-doc consistency, forced SSA, metrics inventory,
  Pingora source policy, Gateway API transform contract, and unit-test layout
  guards: passed.
- Independent read-only deep review by an Astra agent: LGTM after correcting and
  re-reviewing the protocol test's initial-frame completion barrier.

Re-evaluate if Pingora/h2 defaults change, the policy gains runtime updates or
per-listener overrides, or workload evidence requires different bounds. Do not
present the configuration budget as a global memory cap.

## Resolution references

- `P090-010` — operator configuration enhancement; accepted process-only scope,
  unchanged defaults, and focused testing in this run.
