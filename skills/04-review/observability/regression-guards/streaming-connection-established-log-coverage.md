---
name: streaming-connection-established-log-coverage
description: Use when reviewing immediate WebSocket or gRPC-Web access-log delivery and its integration evidence.
knowledge_type: regression-guards
---

# Streaming connection-established access-log coverage

## Conclusion and scope

An integration assertion for the immediate connection-established access log must
observe the configured output while the connection remains usable. A terminal
AccessLogStore query does not establish this property.

The gateway's existing `connected_to_upstream` callback emits an
`AccessLogEntry` with `conn_est: true` for detected `websocket` and `grpc-web`
requests through `AccessLogger::send`. This marker means the upstream connection
was established; it does not promise that a later WebSocket upgrade succeeds.
The [HTTP lifecycle](../../../01-architecture/02-gateway/02-pingora-lifecycle.md)
documents this checkpoint. Production behavior and structures are unchanged.

## Rationale and evidence

- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs` detects the
  protocol into `RequestInfo.discover_protocol` and reads or generates
  `RequestInfo.x_trace_id` (serialized as `request_info.x-trace-id`).
- `edgion-gateway/src/routes/http/proxy_http/pg_connected_to_upstream.rs` calls
  `AccessLogEntry::set_conn_est` and sends the canonical JSON directly.
- `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs` writes the optional
  integration AccessLogStore only at termination. Its separate final entry does
  not carry the connection-established marker.
- The former WebSocket suite covered echo and response-stage upgrade refusal,
  but neither asserted the immediate log. Losing or delaying that send could
  therefore leave those tests passing.
- [`websocket_conn_est_access_log`](../../../../edgion-tests/src/client/suites/http_route/protocol/websocket.rs)
  tails `TestContext.access_log_path` from the pre-request EOF, correlates a
  fresh UUID trace ID, and requires boolean `conn_est: true` plus protocol
  `websocket`. It then sends a message and requires its echo on the same socket
  before attempting close, excluding teardown-only output as passing evidence.

## Alternatives and limits

The case reuses the existing shared topology, WebSocket fixtures, and suite
registrations. It runs only in local gateway mode, through both the targeted
`HTTPRoute/Protocol/WebSocket` selection and the existing full-sweep WebSocket
registration. No new process, log sink, production field, or dependency is added.
User documentation and hot-reload behavior are unaffected.

The existing `EDGION_TEST_K8S_MODE` flag excludes this additional case because
Kubernetes test-client Pods do not mount the gateway log; both pre-existing
WebSocket cases remain registered there. A missing file in local mode fails
the new case rather than silently skipping it.

The configured LocalFile output must be readable by the test client. This is
local integration coverage, not evidence for a remote gateway without a mounted
log file, every log sink, gRPC-Web wire behavior, retries, or log rotation. The
reader pins the active file for this short case; rotation during the observation
window can cause a bounded failure and requires rerunning against a stable file.
Partial lines are retained until their newline arrives. The scan reads at most
1 MiB of new bytes, and failure messages never dump unrelated log contents.

Handshake is limited to 5 seconds, log observation plus the post-log echo to
10 seconds, and close to 2 seconds. The socket is dropped even if close fails;
the framework also owns a 30-second case deadline. Missing output, malformed
matching JSON, an absent/wrong marker, or an unusable socket cannot pass.

## Verification and re-evaluation

Build before starting the shared topology, then run the targeted suite:

```bash
bash edgion-tests/integration/scripts/utils/prepare.sh
bash edgion-tests/integration/scripts/integration/run_integration.sh \
  --no-prepare -r HTTPRoute -i Protocol/WebSocket
```

Fresh local verification on 2026-09-09:

- Canonical `prepare.sh` passed; after the K8s registration restriction,
  `cargo build -p edgion-tests --bin test_client` rebuilt the final client.
- The targeted runner passed on the final revision: 3/3 WebSocket cases,
  including `websocket_conn_est_access_log`, with zero infrastructure errors.
  The shared-stack preflights and final metrics-label, unknown-field, requeue,
  and cache-residue gates passed; processes were cleaned up. Run evidence is in
  `tmp/integration_testing/testing_20260909_160759/report.log` and its
  `test_logs/HTTPRoute_Protocol_WebSocket.log` (local, gitignored artifacts).
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy emitted existing
  warnings in unchanged code and none in the modified WebSocket file.
- Agent-doc consistency, SSA-force, metrics-inventory, and Pingora-source
  guards passed, as did `git diff --check` and independent read-only review.
- Kubernetes exclusion was reviewed against both K8s runner exports, not
  executed against a cluster. No full workspace test run was performed.

Re-evaluate when immediate log timing, protocol discovery, trace serialization,
sink configuration/rotation, or WebSocket suite registration changes. Preserve
the post-observation live-socket assertion when replacing the log reader.

## Resolution references

- `add-streaming-conn-est-access-log-integration.md`, created 2026-09-09, P3:
  a real integration coverage gap, addressed by the gateway-only WebSocket case.
- B0178 found no production change necessary; its historical unit-test results
  did not verify immediate configured-output delivery on a live connection.
- The user authorized the existing WebSocket test-file edit in the single-issue
  repair workflow; the earlier campaign's frozen integration scope was retained
  for unrelated test files.
