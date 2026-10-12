---
name: body-admission-reads-pre-plugin-snapshots
description: Use when reviewing body-gate admission findings; covers why the silent PassThrough disposition requires a live plugin-mutable fact AND its pre-plugin ctx snapshot to agree, while Refuse steps may read the live fact alone.
status: fixed
finding_id: response-body-gate-sync-stage F3 (with F5)
closed: 2026-07-26
knowledge_type: regression-guards
---

# The silent admission disposition requires unanimous facts — FIXED

## Conclusion

This page describes the adapter's admission contract, not a supported full-response
body API. `DslConfig::compile_and_validate` rejects `BuiltinId::RespBody` in both
source and bytecode configurations. The DSL reproductions below are historical;
the main upstream response remains streaming under `AGENTS.md`'s HTTP body
invariant 2. The adapter's `Buffer` disposition currently returns
`BodyReadError::Unimplemented`; it does not enable response buffering.

`request_admission` and `response_admission`
(`edgion-gateway/src/plugins/runtime/session_adapter.rs`) never let a
plugin-mutable fact steer toward a *silent* disposition. `PassThrough` is the
only silent disposition. `request_admission` also returns it (step 1,
`body_declared == Some(false)`), but that input is safe by construction —
`snapshot_body_declared` is its only writer, no live counterpart is consulted,
and no plugin API can make a declared-empty request body non-empty. That leaves
`response_admission` step 4 as the only place a mutable fact could reach a
silent answer, so **both** of its inputs are pre-agreed by the caller:

| input | live (mutable) fact | pre-plugin snapshot | mutated by |
|---|---|---|---|
| `is_head` | `req_header().method` | `RequestInfo::method_is_head()` | `set_upstream_method` |
| `bodyless_agreed` | bound response header status | `ctx.pre_plugin_response_status` | `set_response_status` |

Each is the AND of the two columns. After the refusal checks, `PassThrough`
requires either the HEAD pair or the bodyless-status pair to agree; disagreement
in one pair alone does not defeat agreement in the other. If neither pair agrees,
the result is `Buffer`, which fails loudly. Steps 1–3 deliberately keep reading the live status
alone, because they all steer toward `Refuse` — the loud direction, which a
mutated fact may safely widen.

Fix suggestions of the form "just use the live value — that is what the upstream
actually produced" and "just use the snapshot — the live one is plugin-mutable"
are **both not accepted** for either input: each alone reopens one half of the
hole (see Core Rationale 1).

## Core Rationale

**1. Both single-fact readings are broken, in opposite directions**

`RequestAdmission::PassThrough` resolves to `Ok(Bytes::new())`, and the
contract of that return is *"the response truthfully declared no body"* — a
body-inspecting plugin runs against it and concludes there was nothing to
inspect. `set_upstream_method` (`session_adapter.rs`) rewrites the live header
in place and is reachable from `proxy_rewrite`
(`plugins/http/proxy_rewrite/plugin.rs`) and from DSL's `req.set_method`
(`plugins/http/dsl/lang/builtins.rs`, which is **not** stage-gated, so it is
callable from `upstreamResponseFilterPlugins` too). Crucially, `self.inner` is
the *downstream* `pingora_proxy::Session`, so that rewrite also changes what
pingora itself sees.

- *Live header alone* (the original bug): a rewrite TO `HEAD` — including one
  performed at the response stage, after the upstream body already arrived —
  makes the read report "no body".
- *Snapshot alone* (the mirror hole, caught in review before it shipped): a
  client sends `HEAD`, a request-stage plugin rewrites the method to a
  body-carrying one, so the upstream returns a real body **and pingora writes
  it downstream** — `init_body_writer`
  (`pingora-core/src/protocols/http/v1/server.rs`) keys on
  `self.get_method() == Some(&Method::HEAD)`, i.e. the same live header the
  rewrite mutated. The snapshot still says HEAD, so the consumer sees nothing
  while the client sees the body.

Note the asymmetry in blame: a *request-stage* rewrite to `HEAD` is not
itself a bypass (the upstream genuinely receives `HEAD` and returns no body).
The load-bearing cases are the response-stage rewrite to `HEAD` and the
rewrite away from `HEAD`.

The **status** input has the same two-direction disagreement shape. `set_response_status`
(`session_adapter.rs`) writes straight into the bound response header, and is
reachable from DSL's `resp.set_status` (`plugins/http/dsl/lang/builtins.rs`)
and from `response_rewrite`'s `status_code`
(`plugins/http/response_rewrite/plugin.rs`), both at the
`upstreamResponseFilterPlugins` stage. In the historical reproduction, before
configuration validation rejected `resp.body`, `resp.set_status(204)` followed by
`resp.body` — or a `response_rewrite {status_code: 204}` entry ordered ahead of
a DSL `resp.body` entry in the same array — could hand the consumer
`Ok(Bytes::new())` for a response whose body is real and already on its way to
the client if admission trusted only the live status.

The current snapshot is `EdgionHttpContext::pre_plugin_response_status`, an
`Option<u16>` that is never serialized. `upstream_response_filter` calls
`select_normalized_upstream_candidate` after response normalization and before
the response plugin chain, so the snapshot holds the normalized, accepted
response status, not necessarily the raw provider status. `None` cannot establish
bodyless-status agreement. `ctx.request_info.status` instead belongs to terminal
downstream/access-log finalization and must not serve as this safety snapshot;
see [Final downstream/access-log status](../../../03-coding/observability/00-access-log.md#final-downstreamaccess-log-status).

**2. The direction of the influence is what matters, not whether the fact is trustworthy**

`request_admission` deliberately DOES read a client-controlled fact
(`is_grpc_request`) — see its doc comment. That is safe because the fact can
only ever *widen a refusal*, and a refusal is unconditionally fail-close (design
R5): it can never be softened into a skip. ANDing each pair gives the response
side the mirror property — a mutable fact can only ever *narrow* `PassThrough`,
never widen it. When reviewing any new admission input, ask which disposition a
hostile or merely-careless value steers toward, not whether the value is
"trusted"; if the answer is `PassThrough`, it needs a snapshot to AND against.

**3. The canonical entry method derives a safe HEAD default**

`RequestInfo::method_is_head()` derives a Boolean from the sole stored
`method: Option<Method>`. Missing entry metadata returns `false` (= not
HEAD = `Buffer` = an attempted read that can fail loudly); it never substitutes
the live method. This preserves the former stored HEAD flag's safe default and
`in_request_body_stage`'s "default to the safe direction" precedent. CONNECT is
also derived from this method, with missing metadata keeping physical-cap
enforcement enabled. `body_declared` remains `Option<bool>` because *its* false
happens to be the unsafe direction. No extra Boolean snapshots remain.

**4. (F5) A bug signal must not share an `EdgionErrorCode` with expected traffic**

`request_admission`'s `body_declared == None` arm means the pre-plugin snapshot
never ran — a code-path defect. Its wrong-stage arm fires on every request for
any body consumer placed in a post-request stage — expected traffic. They used
to return the same `BodyRefusedStage`, making the defect statistically invisible
in the access log. They are now `BodyRefusedNoSnapshot` and
`BodyRefusedStage` respectively. Both `EdgionErrorCode` doc comments state which
one is alertable.

## Fix Suggestions Not Accepted

- "Drop the snapshot, use the live value — it reflects what the upstream
  actually produced" — reopens the rewrite-to-`HEAD` / rewrite-to-bodyless half.
- "Drop the live check, the snapshot is the trustworthy one" — reopens the
  rewrite-away half, where the client demonstrably receives a body.
- "`response_admission` should just match on the status itself instead of taking
  a `bodyless_agreed` flag" — that is the pre-fix shape; the agreement is the
  whole point and belongs at the call site, which is the only place with access
  to both the bound header and the ctx snapshot.
- Passing `bodyless_agreed: is_bodyless_status(status)` from a new call site —
  the single-fact form wearing the new parameter's name, and the most likely way
  this regresses. `is_bodyless_status` is module-private precisely so this is not
  reachable; a new caller must build the AND from both facts, or not call the
  predicate at all.
- "Make `method_is_head()` return `Option<bool>` for symmetry with `body_declared`" —
  adds a third refusal path for no gain; `false` is already the safe default
  here.
- "Give `response_admission` the whole `Session` so it can decide for itself" —
  the predicate is pure and unit-testable precisely because it takes only ctx
  facts; the ordering of its steps IS the semantics and needs to stay testable
  without a live `Session`. The live-header AND lives at the
  `read_response_body` call site for the same reason.
- "Snapshot the method in the plugin runtime instead of `snapshot_body_declared`" —
  the runtime already runs after plugins can mutate; `snapshot_body_declared`
  (`routes/http/proxy_http/pg_request_filter.rs`) runs after early RequestId-only
  handling and Pingora modules, before routing and ordinary rewrite plugins.

## Re-evaluation Triggers

Re-evaluate when:

- A new admission input is proposed whose mutated value steers toward
  `PassThrough` or `Buffer` rather than `Refuse` — re-check the direction
  argument in Core Rationale 2 before accepting it.
- Pingora stops deriving the downstream body-framing decision from the same
  mutable request header (`init_body_writer`), which would make the live-header
  half of the HEAD AND unnecessary.
- `pre_plugin_response_status` gains a writer, becomes plugin-mutable, or its
  capture moves across response normalization or plugin execution. Recheck retry
  and accepted-response ownership as well; terminal `request_info.status` is not
  a substitute.
- Either response entry point stops skipping interim `1xx` headers (excluding
  terminal `101`). Historical asymmetry was closed on 2026-08-03: both
  `pg_upstream_response_filter` and `pg_response_filter` now use
  `is_interim_informational` before their plugin runners. Recheck this guard if
  admission becomes reachable on an interim header — see
  [[response-stage-denial-must-terminate]].
- A response-body consumer or DSL validation change is proposed. Recheck the
  streaming-only authority first; this historical admission fix does not
  authorize full-response aggregation or DSL `resp.body` support.
- A code path is added that reaches `read_response_body` without passing
  `snapshot_body_declared` — today every such path goes through
  `build_request_metadata`, whose first statement is that snapshot.

## Reference Cases

- `response-body-gate-sync-stage` Findings 3 and 5, fixed 2026-07-26. The rest
  of that cluster is now also closed and the task file is deleted: Finding 2
  (`check_response_stage_terminate`'s refused-reply branch not terminating) was
  fixed 2026-08-03 — see [[response-stage-denial-must-terminate]]. Finding 1
  (the ungated sync `upstreamResponseFilterPlugins` array) was retired: DSL's
  `resp.body` is rejected at configuration validation, so there is no
  DSL response-body consumer to gate. Earlier references to body-processing work
  as a way to make `resp.body` expressible are obsolete under the current
  streaming-only product boundary.
- Design: `docs/superpowers/specs/2026-07-23-body-gate-admission-model-design.md`
  (R5 refusal semantics).
- Source: `edgion-gateway/src/plugins/runtime/session_adapter.rs`
  (`request_admission`, `response_admission`, `read_response_body`),
  `edgion-gateway/src/ctx.rs` (`RequestInfo::method_is_head()`,
  `EdgionHttpContext::pre_plugin_response_status`),
  `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`
  (`select_normalized_upstream_candidate`),
  `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs`
  (`DslConfig::compile_and_validate`),
  `edgion-resources/src/diagnostic.rs` (`BodyRefusedNoSnapshot`).

## Documentation verification

D0144 (`correct-response-body-admission-snapshot-documentation`) corrects
documentation drift only, not a production regression. Source inspection confirms
the separate snapshot, its normalized capture point, and the live-plus-snapshot
agreement. Existing tests `read_response_body_ignores_plugin_rewritten_bodyless_status`
and `read_response_body_ignores_bodyless_snapshot_when_status_rewritten_away`
cover both status rewrite directions; DSL's
`validation_error_rejects_streaming_upstream_response_body_read` covers configuration
rejection. These tests were read, not rerun for this documentation-only correction.
The historical fix dates below are not fresh runtime verification.

For this correction, independent read-only review returned LGTM. `cargo fmt --all`,
`cargo check --workspace --all-targets`, and `cargo clippy --workspace --all-targets`
completed successfully; Clippy reported warnings in unchanged Rust files. The
agent-doc, SSA force, metrics inventory, and Pingora source policy guards passed.
The initial agent-doc check found an unrelated missing-task link in the listener
admission knowledge page; its reference was repaired while preserving the UDP
documentation conflict as unresolved, and the check then passed. No runtime or
integration-test files changed, and no full test suite was run.

## Resolution references

- `response-body-gate-sync-stage F3+F5` — 2026-07-26.

## Entry-method consolidation

T01 of [request identity](../../../../tasks/pending/http-request-identity/http-request-identity.md)
replaces stored HEAD/CONNECT booleans with queries derived from the canonical
entry Method; the live-plus-entry AND and missing HEAD false policy are preserved.
Existing HEAD test identities remain, including both rewrite directions. The
new missing-entry/live-HEAD test explicitly rejects silent admission. This is an
ownership consolidation, not permission to buffer main upstream responses.
Fresh verification and independent result review are recorded in the task; do not
infer their completion from this architectural description.
