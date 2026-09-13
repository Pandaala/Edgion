---
name: terminate-explicit-local-response
description: Review DSL terminate decisions, local response ownership, and the distinction from the bare termination bug fallback.
knowledge_type: regression-guards
---

# DSL terminate uses an explicit local response

## Conclusion and scope

`return terminate()` executes `OpCode::ReturnTerminate`, which returns
`PluginRunningResult::ErrResponse { status: 502, body: None }`. `status` selects
the framework-owned rejection; `body: None` specifies an empty HTTP body.
Source and validated precompiled bytecode share this branch. This is normal
control flow, so even `DslErrorPolicy::Ignore` cannot allow the request through.

`execute_safe` records exactly one `Deny 502` decision. The shared response
dispatcher can append its own replacement or denial record. A valid terminate
does not produce `PluginTerminateWithoutResponse`; other diagnostics still
describe actual response-stage limitations or delivery failure.

## Rationale and evidence

[Design rule F](../../../00-design-rules.md#f-failure-policy--error-replies)
requires ordinary plugin rejection to use `ErrResponse`; `ErrTerminateRequest`
is reserved for inherently dynamic responses already written by the plugin.
The old VM returned the latter without writing anything. The request-stage
`terminate_with_fallback` and response-stage
`set_response_terminate_without_reply` therefore treated a supported script
decision as a plugin bug. Fix the VM producer; keep those global safeguards.

Current call chain:

- `edgion-dsl/src/compiler/stmt.rs`: `ReturnTerminate` opcode emission.
- `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`: explicit response decision
  and the successful `execute_safe` branch.
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`: request writer and
  synchronous response-stage chain, both stopping after the decision.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`:
  `build_response_local_reply` and `set_response_denied` own replacement/abort.
- `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs` and
  `pg_fail_to_proxy.rs`: consume the staged reply or denial downstream.

## Alternatives and limits

The chosen 502 preserves the status observed by existing HTTP integration
scenarios; those tests were evidence, not authority for a DSL exception to F.
The HTTP body changes from fallback HTML to empty. Normal local-response header
rules apply: request-phase queued response operations are drained and framing
is reasserted; response-phase HTTP replacement discards upstream headers/body
and carries queued request-phase response operations. Response-phase
`resp.set_header()` edits the bound upstream header directly, so those edits
are discarded with that header rather than copied to the local response.
HEAD remains bodyless.

Request-stage gRPC maps 502 to HTTP 200 / INTERNAL (13). Response-stage gRPC
cannot accept an HTTP local replacement: it retains
`PluginResponseReplaceUnsupportedGrpc` and `PluginResponseDenyAborted`, arms
`ResponseStageDenial::Http(502)`, and reaches the existing HTTP-to-gRPC writer
through `fail_to_proxy`. Genuine 101 and established CONNECT also take the
denial path. Merely requesting Upgrade, or a rejected CONNECT, does not
prevent ordinary replacement. No new full-response buffering is introduced.

HTTP/1 response-stage local replacement still closes the downstream connection;
see [the existing lifecycle constraint](../../http-body/intentional-design/response-local-reply-h1-close-is-feature.md).
There is no silent-close API or connection-reuse promise.

Empty HTTP/2 local replies must also send EOS before the response-stage sentinel
aborts the pump. `Session::write_response_header(_, true)` alone does not pass
its end flag into the pinned Pingora H2 codec. `pg_response_filter` explicitly
writes `write_response_body(None, true)` after marking response ownership and
before recording successful delivery. Otherwise session drop resets the stream.

The shared gRPC terminal writer owns both `grpc-status` and `grpc-message`:
it removes plugin-queued values before selecting the native or gRPC-Web wire
shape. See [terminal status ownership](../../h2-grpc/regression-guards/terminal-status-field-ownership.md).

The historical [iteration-control page](../../plugin-runtime/compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md)
still describes response replacement as unavailable. That paragraph is stale
against `build_response_local_reply`; its sync/async bridge discussion does not
invalidate the replacement mechanism used here. Broader maintenance of that
historical page is outside this fix's frozen scope.

## Verification and re-evaluation

Deterministic coverage retains all four original VM cases and exact decision-log
assertions, adds error-policy independence, and drives real DSL plugins through
the shared runtime with in-memory duplex sessions. Additional codec tests drive
real DSL source and bytecode through H2 response replacement and an active
GrpcWebBridge. They verify clean H2 EOF after response-filter/fail-to-proxy/drop
and the exact single gRPC-Web trailer frame. They do not run the complete
upstream pump, cross-process/native-gRPC H2 interoperability, or hot reload.

Initial verification on 2026-09-13 (before the protocol corrections):

- Twenty selected tests passed with
  `cargo test -p edgion-gateway --lib -- dsl_terminate terminate_tests decision_log_tests terminate_fallback naked_terminate accepted_local_reply refused_local_reply response_stage_grpc_error`.
  This includes all five VM termination cases, exact decision logs, all four
  runtime wiring tests, and existing response-denial/fallback regressions.
- `cargo check --workspace --all-targets` passed.
- `cargo clippy --workspace --all-targets` passed with existing workspace warnings.
- Format, SSA-force, metrics-inventory, Pingora-source, Gateway no-tracing,
  and scoped validation of all four changed knowledge files passed.
- Global `python3 cicd/checks/validate_agent_docs.py` failed on seven existing
  broken task links in unrelated startup/shutdown, ExtProc, listeners,
  observability, and routing pages. The selected task remains active until the
  mandatory global check passes; scoped validation does not waive that gate.
- Initial plan and code reviews returned LGTM. A subsequent independent Astra
  review superseded that verdict by finding the missing H2 EOS and conflicting
  gRPC-Web status fields. Those are correctness defects, not waived E2E gaps.

Protocol correction verification on 2026-09-13:

- Before the corrections, all three new discriminating tests failed at the
  intended assertions: H2 returned `Reset(StreamId(1), CANCEL, Remote)`, the
  gRPC-Web initial header retained `grpc-status`, and a native reply with no
  canonical message retained a queued `grpc-message`.
- After correction, the 38-test contract/response-filter selection passed,
  including all four new protocol regressions. The 81-test selection covering
  all `runtime::server::error_response::tests` and
  `plugins::runtime::session_adapter::record_outcome_tests` also passed (these
  selections overlap).
- Fresh format, workspace check/clippy, SSA-force, metrics-inventory,
  Pingora-source, Gateway no-tracing, and scoped validation of all six changed
  knowledge files passed. Clippy retains existing workspace warnings. Global
  agent-doc validation still fails only on the seven unrelated links above.
- The independent correction-plan review and a fresh independent Astra
  implementation/knowledge review returned LGTM. The final reviewer inspected
  the pinned fork and test driver ordering without rerunning Cargo; test results
  above were produced by the implementing agent.

Existing integration files are frozen by the selected task. New E2E and dynamic
coverage remains tracked in [the follow-up task](../../../../tasks/todo/dsl-terminate-explicit-response-integration.md).
Re-evaluate if the terminate result, bytecode validation, phase dispatch,
response ownership, or HTTP-to-gRPC mapping changes.

## Resolution references

- `reconcile-dsl-terminate-contract`: confirmed runtime/contract defect; user
  approved the explicit 502/empty-body implementation and independent review
  on 2026-09-13. The runtime correction is implemented; task closure remains
  subject to the verification gate above.
