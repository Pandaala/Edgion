---
name: body-admission-reads-pre-plugin-snapshots
description: Use when reviewing body-gate admission findings; covers why the silent PassThrough disposition requires a live plugin-mutable fact AND its pre-plugin ctx snapshot to agree, while Refuse steps may read the live fact alone.
status: fixed
finding_id: response-body-gate-sync-stage F3 (with F5)
closed: 2026-07-26
---

# The silent admission disposition requires unanimous facts — FIXED

## Conclusion

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
| `is_head` | `req_header().method` | `RequestInfo::method_is_head` | `set_upstream_method` |
| `bodyless_agreed` | bound response header status | `ctx.request_info.status` | `set_response_status` |

Each is the AND of the two columns; any disagreement falls through to `Buffer`,
which can only fail loudly. Steps 1–3 deliberately keep reading the live status
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

The **status** input has the identical shape and is the easiest of the three to
trigger — one script, two lines. `set_response_status`
(`session_adapter.rs`) writes straight into the bound response header, and is
reachable from DSL's `resp.set_status` (`plugins/http/dsl/lang/builtins.rs`)
and from `response_rewrite`'s `status_code`
(`plugins/http/response_rewrite/plugin.rs`), both at the
`upstreamResponseFilterPlugins` stage. `resp.set_status(204)` followed by
`resp.body` — or a `response_rewrite {status_code: 204}` entry ordered ahead of
a DSL `resp.body` entry in the same array — would hand the consumer
`Ok(Bytes::new())` for a response whose body is real and already on its way to
the client. The upstream's own status is available as `ctx.request_info.status`,
stamped in `upstream_response_filter` before any response-stage plugin runs.

**2. The direction of the influence is what matters, not whether the fact is trustworthy**

`request_admission` deliberately DOES read a client-controlled fact
(`is_grpc_request`) — see its doc comment. That is safe because the fact can
only ever *widen a refusal*, and a refusal is unconditionally fail-close (design
R5): it can never be softened into a skip. ANDing each pair gives the response
side the mirror property — a mutable fact can only ever *narrow* `PassThrough`,
never widen it. When reviewing any new admission input, ask which disposition a
hostile or merely-careless value steers toward, not whether the value is
"trusted"; if the answer is `PassThrough`, it needs a snapshot to AND against.

**3. The safe default belongs in the field, not at the call site**

`RequestInfo::method_is_head` is a plain `bool` defaulting to `false` (= not
HEAD = `Buffer` = an attempted read that can fail loudly), matching
`in_request_body_stage`'s "default to the safe direction" precedent.
`body_declared` needs `Option<bool>` only because *its* `false` happens to be
the unsafe direction. Pick the representation from which default is safe, not
by symmetry with the neighbouring field.

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
- "Make `method_is_head` an `Option<bool>` for symmetry with `body_declared`" —
  adds a third refusal path for no gain; `false` is already the safe default
  here.
- "Give `response_admission` the whole `Session` so it can decide for itself" —
  the predicate is pure and unit-testable precisely because it takes only ctx
  facts; the ordering of its steps IS the semantics and needs to stay testable
  without a live `Session`. The live-header AND lives at the
  `read_response_body` call site for the same reason.
- "Snapshot the method in the plugin runtime instead of `snapshot_body_declared`" —
  the runtime already runs after plugins can mutate; `snapshot_body_declared`
  (`routes/http/proxy_http/pg_request_filter.rs`) is the one point guaranteed to
  precede every plugin.

## Re-evaluation Triggers

Re-open only if:

- A new admission input is proposed whose mutated value steers toward
  `PassThrough` or `Buffer` rather than `Refuse` — re-check the direction
  argument in Core Rationale 2 before accepting it.
- Pingora stops deriving the downstream body-framing decision from the same
  mutable request header (`init_body_writer`), which would make the live-header
  half of the HEAD AND unnecessary.
- `ctx.request_info.status` gains a writer that runs BEFORE the response-stage
  plugin runners (today the only pre-runner write is
  `pg_upstream_response_filter.rs`'s stamp of the upstream's own status; the two
  plugin-reachable writes are gRPC-only and step 3 refuses gRPC first).
- The interim-`1xx` asymmetry becomes reachable. `pg_upstream_response_filter.rs`
  early-returns for an interim `1xx` (non-101) BEFORE stamping the status, but
  `pg_response_filter.rs` has no matching guard and pingora calls
  `response_filter` for every header task including `1xx` — so at the ASYNC
  runners on an interim response the snapshot is unset (or holds a prior
  attempt's value after a retry). Today nothing reaches `read_response_body`
  there (the async array takes only `ExtProc`, whose `body_requirement()` is
  `None`); it goes live with the F1 gate. The unset case fails toward `Buffer`,
  the loud direction, so it is a false-loud rather than a bypass. Closed
  2026-08-03: `pg_response_filter` now shares `is_interim_informational` with
  the sync entry point and early-returns, so the async runners no longer execute
  on an interim header at all — see [[response-stage-denial-must-terminate]].
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
  response-body consumer to gate; the residue moved to
  `tasks/todo/llm-ai-gateway/01-body-processing-runtime.md`, which is what would
  make `resp.body` expressible in the first place.
- Design: `docs/superpowers/specs/2026-07-23-body-gate-admission-model-design.md`
  (R5 refusal semantics).
- Source: `edgion-gateway/src/plugins/runtime/session_adapter.rs`
  (`request_admission`, `response_admission`, `read_response_body`),
  `edgion-gateway/src/ctx.rs` (`RequestInfo::method_is_head`),
  `edgion-resources/src/diagnostic.rs` (`BodyRefusedNoSnapshot`).
