---
name: debug-access-log-wire-and-lifecycle
description: Use when reviewing debug access-log response masking, terminal-only fields, or plugin publication coverage.
knowledge_type: regression-guards
---

# Debug access-log wire and plugin lifecycle coverage

## Conclusion and scope

The client-visible `X-Debug-Access-Log` header requires wire assertions in addition
to terminal AccessLog Store assertions. The standalone
[`test_debug_access_log_wire.sh`](../../../../edgion-tests/integration/scripts/integration/test_debug_access_log_wire.sh)
exercises real Controller publication, mTLS configuration sync, Gateway execution,
and HTTP/1.1 responses. Replace and Wrap run serially in separate Gateway processes
and work directories because `SensitiveMaskConfig.mode` is startup-global.

The runtime schema and behavior are unchanged. The
[canonical snapshot contract](../../../03-coding/observability/00-access-log.md)
still owns masking, canonical terminal-log ownership, and the non-terminal snapshot.
The [D0088 development-surface decision](../intentional-design/debug-access-log-development-surface.md)
still owns the accepted size/schema limitations; this coverage does not fix them.

## Rationale and evidence

- `plugins/runtime/gateway_api_filters/debug_access_log.rs` in `edgion-gateway`
  suppresses Wrap before constructing the snapshot; other modes serialize
  `AccessLogEntry::from_context` and set the response header.
- `observe/access_log/entry.rs` initializes `waf: None` and empty `ext`.
  Terminal `routes/http/proxy_http/pg_logging.rs` adds the final fields and
  stores the same canonical serialized entry in the integration AccessLog Store.
- The previous DebugAccessLog suite only reads that terminal store. Its fixture
  does not enable `DebugAccessLogToHeader`; the two filter unit tests use a mock
  session and cannot prove Controller publication or actual response headers.
- The new scenario uses a matching detection-only WAF rule and an
  `accessLogExtern` route-label field as positive controls: both exist in the
  terminal record while neither appears in the debug snapshot.
- Replace requires exactly one JSON-object header (including duplicate-key
  rejection), trace correlation, masked `direct_peer_ip`, `remote_ip`, `path`,
  and explicitly enabled `request_uri` (`access_log_options.includeRawUri: true`),
  plus unfinished status `0`.
- Wrap requires no debug header and exactly one
  `wrap mode:header suppressed` decision for each active plugin execution.
  Both modes preserve the backend's `201` and `Status: 201` body in serving phases.

## Lifecycle and verification limits

Each mode checks baseline, resource add, entry disable, entry removal,
re-enable, resource deletion, and recreation. Updates use the Controller Admin
API, then wait for the same `resourceVersion` in Gateway `/configclient` and a
response generation marker. The terminal log independently checks whether the
debug plugin executed. Resource deletion waits for absence from the Gateway
cache and asserts the existing dangling-ExtensionRef `500` with no debug header;
recreation restores service. Resource transitions never restart either process.

This is a directly invoked standalone scenario, not registered in the frozen
existing integration runners. It preserves existing test files and generates
all fixtures under its owned run directory. It uses the existing `test_server`,
canonical conf_sync certificate helpers, dynamic ports, bounded state polling,
and the shared whole-run process-group deadline. The shell entry point is the
supported invocation; direct Python invocation omits the whole-run deadline.

HTTP/2, intermediary header budgets, Off mode, oversized/header-set failures,
and mid-request generation pinning are outside this scenario's evidence.
No production configuration, runtime code, or user-visible contract changed;
user documentation therefore needs no update.

## Verification and re-evaluation

Build current binaries before the test; the scenario never builds at runtime:

```bash
cargo build -p edgion-gateway -p edgion-controller -p edgion-tests \
  --bin edgion-gateway --bin edgion-controller --bin test_server
bash edgion-tests/integration/scripts/integration/test_debug_access_log_wire.sh
```

Override binary paths with `EDGION_GATEWAY_BIN`, `EDGION_CONTROLLER_BIN`,
`EDGION_TEST_SERVER_BIN`, or `--bin-dir`. `--state-timeout` defaults to 30 seconds;
`EDGION_DEBUG_WIRE_TIMEOUT_SECONDS` defaults to a 300-second whole-run deadline.
The printed `tmp/integration_testing/debug-access-log-wire.*` directory retains
process logs, generated inputs, ports, and each successful phase's wire/terminal
JSON evidence on both success and failure.

Fresh verification on 2026-09-09: current Controller/Gateway/test_server builds
and all 14 wire/lifecycle checks passed on macOS arm64, including the explicitly
enabled raw-URI projection. `bash -n`, Python bytecode compilation, and the Bash
increment guard passed. The shell entry point's syntax and `--help` execution
also passed in a Linux Bash 5.2 container; Linux wire behavior was not tested.
Required closure checks passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`
(warnings in unchanged Rust code), and the agent-doc, SSA-force, metrics-inventory,
and Pingora source-policy guards. The English-only guard also passed.

Additional timeout injection on macOS exposed a pre-existing shared-wrapper
limitation: after terminating the group, `os.killpg(pid, 0)` raised EPERM and
the wrapper returned 1 rather than 124. The same failure reproduced with a
minimal Python sleep command, without this scenario. No scenario processes
remained in the subsequent process listing. The cause and robust cross-platform
handling remain tracked in
[ITEST-DEADLINE-EPERM](../../../../tasks/todo/investigate-integration-deadline-group-probe-eperm-on-macos.md);
the frozen shared helper is unchanged. This is a real unresolved limitation,
not an accepted design or a passing timeout-classification check.
The minimal shared-wrapper timeout reproduction returned the expected 124 in
the Linux container; this limits the observed failure to the tested macOS
environment without claiming a fully diagnosed platform cause.

Re-evaluate when masking startup ownership, snapshot fields, response-header
mutation, plugin publication, or dangling-reference policy changes. A future
shared-runner registration must retain mode isolation and needs authorization
to change the frozen runner wiring.

## Resolution references

- D0087, originally medium / needs-implementation in B0125: a real cross-process
  test gap, addressed with standalone wire and resource-lifecycle coverage.
- Source task: `tasks/todo/add-debug-access-log-wire-integration-coverage.md`.
