---
name: response-stage-denial-must-terminate
description: Use when reviewing response-stage plugin denials, local-reply refusal predicates, or Upgrade/CONNECT/gRPC/101 handling; covers why the refusal must be decided from the response and why a refused reply must still terminate the exchange.
status: fixed
finding_id: response-body-gate-sync-stage F2
closed: 2026-08-03
---

# A response-stage denial is decided from the RESPONSE, and a refused reply still terminates — FIXED

## Conclusion

Two independent rules govern every terminal result a response-phase plugin can
return — `ErrResponse`, `ErrGrpcError`, `ErrTerminateRequest` — and violating
either one turns a denial into a silent ALLOW:

1. **Whether the exchange can still carry a replacement response is a fact about
   the RESPONSE, never about the request's stated intent.**
   `apply_response_local_reply`
   (`edgion-gateway/src/plugins/runtime/session_adapter.rs`) refuses only for an
   informational upstream status (`1xx`, which includes a real `101`) or a
   `CONNECT` the upstream answered `2xx`. It must not consult the request's
   `Upgrade` header or bare `CONNECT` method.

2. **"Terminate" must be expressed as state a response-stage reader consults.**
   Whenever the reply cannot be stashed — or was never a reply at all —
   `check_response_stage_terminate`
   (`plugins/runtime/plugin_runtime.rs`) arms `ctx.response_denial`, and both
   response-stage entry points (`pg_upstream_response_filter`,
   `pg_response_filter`) return `Err(HTTPStatus(..))` so `fail_to_proxy`
   delivers the denial and the upstream connection is torn down.

Fix suggestions of the form "narrowing the refusal predicate is enough" and
"the runners already call `set_terminate`" are **both not accepted** — each
alone leaves a live bypass (Core Rationale 1 and 2).

## Core Rationale

**1. The request's `Upgrade` header is intent, not state**

A client may send `Upgrade: <anything>` on an ordinary request. Only `websocket`
is recognized (`pg_request_filter::detect_request_protocol`); every other token
is forwarded and the exchange stays ordinary HTTP. Keying the refusal on the
header's mere presence meant any client could append one meaningless header and
have every response-stage denial dropped — the upstream response was forwarded
unmodified with only a `deny response {status}` log line.

The authoritative fact is the upstream's answer: no `101`, no switch. Nothing
has been written downstream at either entry point either — `response_committed`
is set only by the local-reply and error paths, never by the ordinary forward —
so the response is still in hand and replaceable.

**2. `set_terminate` has no response-stage reader**

`PingoraSessionAdapter::set_terminate` writes only
`ctx.plugin_running_result`. Every reader of that field is request-stage
(`pg_request_filter`, `pg_upstream_peer`, `grpc/integration`);
`pg_response_filter` consults `ctx.local_reply_header` instead. So "log and
`set_terminate`" is indistinguishable from "allow" once the reply was refused.
This is why the residual cases — a real `101`, an established CONNECT tunnel, a
gRPC route — need `ctx.response_denial` and a real abort, not a stronger log.

**3. A naked `ErrTerminateRequest` leaked with no precondition at all**

The `ErrTerminateRequest` arm simply returned `true`. Unlike the `ErrResponse`
case it needed no refusal to leak — every response-stage `terminate()` was
answered by forwarding the upstream response, with no marker in the access log.
It now mirrors the request stage's contract for the same result
(`error_response::terminate_with_fallback`): a 502 fallback plus
`PluginTerminateWithoutResponse`, on top of the shared abort.

**4. `ErrGrpcError` needs its own carrier**

The sync response runner cannot await, so it cannot call
`send_grpc_error_direct` the way the request stage does. Routing the code
through an HTTP status would flatten it (`http_to_grpc_status` is lossy — e.g.
`RESOURCE_EXHAUSTED` has no HTTP preimage and degrades to `INTERNAL`).
`ResponseStageDenial::Grpc` therefore carries the status and message verbatim to
`fail_to_proxy`, which writes the trailers itself before its generic
HTTP-status handling would see them.

**5. Interim `1xx` must not reach the plugins at all**

Once a refused reply arms an abort, running response plugins on a `100`/`103`
becomes actively harmful: `apply_response_local_reply` refuses an informational
response, so a denial there would abort an exchange whose real response has not
arrived. Both entry points share `is_interim_informational` and return early;
`101` is excluded because it is terminal, with no later header to defer to.

## Verified behavior

| Case | Delivered as | Evidence |
|---|---|---|
| `Upgrade: foo` + upstream 200 + `deny(403)` | local reply, 403 | `dsl_resp_deny_survives_unrecognized_upgrade`; access log `replace response 403` |
| real `101` + `deny(403)` | abort, 403, upgrade never completes | `websocket_response_stage_deny_blocks_upgrade`; access log `deny response 403` + `errors:["PluginResponseDenyAborted"]` + `upstream.err:["HTTPStatus"]` |
| CONNECT answered non-2xx | local reply | unit: `local_reply_replaces_rejected_connect` |
| CONNECT answered 2xx / interim 1xx | refused (unchanged) | unit: `local_reply_refused_for_established_connect_tunnel`, `..._interim_informational` |
| naked `terminate()` at the response stage | abort, 502 + `PluginTerminateWithoutResponse` | unit: `response_stage_naked_terminate_arms_the_abort` |
| `grpc_error(8, ..)` at the response stage | abort, exact code 8 | unit: `response_stage_grpc_error_arms_the_abort_with_the_exact_status`; integration blocked, see `tasks/todo/integration-dslgrpcerror-item-never-runs.md` |

## Closed follow-up

`fail_to_proxy` fills `upstream.status` only when no earlier stage recorded a
status. A response-stage denial therefore keeps the backend's real status in
the per-upstream entry (for example, `101`) while the request-level status
describes the denial delivered by the gateway (for example, `403`). The same
rule preserves the observed backend status when retry-budget denial produces a
synthetic `503`; connect and transport failures still receive a status because
their entry has no observed response status to preserve.

Related: [[body-admission-reads-pre-plugin-snapshots]],
[[local-reply-drops-sticky-set-cookie]],
[[filter-iteration-control-sync-phase-tradeoff]].
